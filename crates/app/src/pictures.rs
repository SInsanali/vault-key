//! Weapon pictures for the page: `gk://localhost/picture/<size>/<item code>`.
//!
//! The pictures are drawn by `gamemesh` from the user's own game install (read-only) and kept
//! only in memory. One [`Service`] lives on a thread of its own, so drawing never holds up
//! the window or the API worker. The game's package is opened on the first request; if that
//! fails, the failure is remembered and every later request is answered at once.

use std::collections::HashMap;

use gamemesh::{Facing, PictureError, Pictures, Style};

/// Which way weapons point in every picture. Flip this one constant to compare with the
/// game: neither facing is a mirror image, each shows a different side of the weapon.
pub const FACING: Facing = Facing::MuzzleLeft;

/// Most bytes of finished PNGs kept in memory. A detail picture is 20 to 70 KB and a
/// thumbnail 8 to 25 KB, so this holds several hundred of each.
pub const CACHE_BYTES: usize = 24 << 20;
/// Longest item code accepted, in characters. Real ones are under 100.
const MAX_CODE_LENGTH: usize = 400;

/// The picture sizes the page may ask for. Each is drawn at twice the size it is shown at.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Size {
    /// On an item card: shown at 144 × 72.
    Thumb,
    /// In the detail panel: shown at 264 × 132.
    Detail,
}

impl Size {
    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "thumb" => Some(Self::Thumb),
            "detail" => Some(Self::Detail),
            _ => None,
        }
    }

    /// Width and height in pixels.
    pub fn pixels(self) -> (u32, u32) {
        match self {
            Self::Thumb => (288, 144),
            Self::Detail => (528, 264),
        }
    }
}

/// What to send back for one request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Answer {
    /// The HTTP status: 200 with a PNG, anything else with a sentence for the user.
    pub status: u16,
    pub content_type: &'static str,
    pub body: Vec<u8>,
}

impl Answer {
    fn png(bytes: Vec<u8>) -> Self {
        Self { status: 200, content_type: "image/png", body: bytes }
    }

    fn sentence(status: u16, text: &str) -> Self {
        Self { status, content_type: "text/plain; charset=utf-8", body: text.as_bytes().to_vec() }
    }

    /// Bad request for a code that does not read, not found for things that have no picture,
    /// unavailable when the game's files are the problem.
    fn refusal(error: &PictureError) -> Self {
        let status = match error {
            PictureError::BadCode => 400,
            PictureError::NotAWeapon | PictureError::NothingToDraw | PictureError::DlcItem | PictureError::UnknownParts => 404,
            PictureError::GameData(_) => 503,
        };
        Self::sentence(status, error.message())
    }

    pub fn is_picture(&self) -> bool {
        self.status == 200
    }
}

/// Splits `<size>/<url-encoded item code>` (what follows `picture/` in the address).
pub fn parse_route(route: &str) -> Result<(Size, String), Answer> {
    let (size, code) = route.split_once('/').ok_or_else(|| Answer::sentence(400, "The picture address is incomplete."))?;
    let size = Size::from_name(size).ok_or_else(|| Answer::sentence(400, "There is no picture size of that name."))?;
    let code = percent_decode(code).filter(|c| !c.is_empty() && c.chars().count() <= MAX_CODE_LENGTH);
    code.map(|code| (size, code)).ok_or_else(|| Answer::refusal(&PictureError::BadCode))
}

/// Undoes `%XX` escapes. `None` for a broken escape or bytes that are not UTF-8.
fn percent_decode(text: &str) -> Option<String> {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let hex = bytes.get(i + 1..i + 3)?;
            out.push(u8::from_str_radix(std::str::from_utf8(hex).ok()?, 16).ok()?);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

/// Which of the waiting requests to answer next: the newest detail picture if there is one
/// (the person is looking at that), otherwise the oldest request.
pub fn next_request<T: AsRef<str>>(waiting: &[T]) -> Option<usize> {
    if waiting.is_empty() {
        return None;
    }
    Some(waiting.iter().rposition(|route| route.as_ref().starts_with("detail/")).unwrap_or(0))
}

/// Something that can draw a weapon from its item code.
pub trait Painter: Send {
    fn png(&mut self, code: &str, width: u32, height: u32) -> Result<Vec<u8>, PictureError>;
}

impl Painter for Pictures {
    fn png(&mut self, code: &str, width: u32, height: u32) -> Result<Vec<u8>, PictureError> {
        self.weapon_png(code, width, height)
    }
}

type Opener = Box<dyn FnOnce() -> Result<Box<dyn Painter>, PictureError> + Send>;

/// Finished PNGs by size and code, least recently used dropped first once over the bound.
struct PngCache {
    entries: HashMap<(Size, String), (Vec<u8>, u64)>,
    bytes: usize,
    limit: usize,
    clock: u64,
}

impl PngCache {
    fn get(&mut self, key: &(Size, String)) -> Option<Vec<u8>> {
        self.clock += 1;
        let entry = self.entries.get_mut(key)?;
        entry.1 = self.clock;
        Some(entry.0.clone())
    }

    fn put(&mut self, key: (Size, String), png: &[u8]) {
        // Something too big to keep at all is simply not kept.
        if png.len() > self.limit {
            return;
        }
        self.clock += 1;
        if let Some(old) = self.entries.insert(key, (png.to_vec(), self.clock)) {
            self.bytes -= old.0.len();
        }
        self.bytes += png.len();
        while self.bytes > self.limit {
            let Some(oldest) = self.entries.iter().min_by_key(|(_, entry)| entry.1).map(|(key, _)| key.clone()) else { break };
            if let Some(dropped) = self.entries.remove(&oldest) {
                self.bytes -= dropped.0.len();
            }
        }
    }
}

/// Answers picture requests. Owns the painter (opened on first use) and the PNG cache.
pub struct Service {
    opener: Option<Opener>,
    painter: Option<Result<Box<dyn Painter>, PictureError>>,
    cache: PngCache,
}

impl Service {
    /// Draws from the installed game: found the same way the item catalog finds it, so
    /// `GK_GAME_DATA` is honoured.
    pub fn live() -> Self {
        Self::new(Box::new(|| {
            let install = gamedata::GameInstall::locate()
                .ok_or_else(|| PictureError::GameData("Borderlands 2 was not found".to_string()))?;
            let mut pictures = Pictures::open(&install)?;
            pictures.set_style(Style { facing: FACING, ..Style::default() });
            Ok(Box::new(pictures) as Box<dyn Painter>)
        }))
    }

    pub fn new(opener: Opener) -> Self {
        Self::with_limit(opener, CACHE_BYTES)
    }

    pub fn with_limit(opener: Opener, cache_bytes: usize) -> Self {
        Self { opener: Some(opener), painter: None, cache: PngCache { entries: HashMap::new(), bytes: 0, limit: cache_bytes, clock: 0 } }
    }

    /// Bytes of finished PNGs held right now.
    #[cfg(test)]
    pub fn cached_bytes(&self) -> usize {
        self.cache.bytes
    }

    /// The answer for `<size>/<url-encoded item code>`.
    pub fn answer(&mut self, route: &str) -> Answer {
        let key = match parse_route(route) {
            Ok(key) => key,
            Err(refusal) => return refusal,
        };
        if let Some(png) = self.cache.get(&key) {
            return Answer::png(png);
        }
        if let Some(opener) = self.opener.take() {
            self.painter = Some(opener());
        }
        let painter = match &mut self.painter {
            Some(Ok(painter)) => painter,
            Some(Err(why)) => return Answer::refusal(why),
            None => return Answer::refusal(&PictureError::GameData("the picture source is gone".to_string())),
        };
        let (width, height) = key.0.pixels();
        match painter.png(&key.1, width, height) {
            Ok(png) => {
                self.cache.put(key, &png);
                Answer::png(png)
            }
            Err(why) => Answer::refusal(&why),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    /// Draws `size` bytes for codes starting `BL2(`, counting its calls; other codes are items.
    struct Fake {
        drawn: Arc<AtomicUsize>,
        size: usize,
    }

    impl Painter for Fake {
        fn png(&mut self, code: &str, width: u32, _height: u32) -> Result<Vec<u8>, PictureError> {
            self.drawn.fetch_add(1, Ordering::SeqCst);
            match code {
                "item" => Err(PictureError::NotAWeapon),
                "dlc" => Err(PictureError::DlcItem),
                "parts" => Err(PictureError::UnknownParts),
                "empty" => Err(PictureError::NothingToDraw),
                "broken" => Err(PictureError::GameData("a mesh".into())),
                c if c.starts_with("BL2(") => Ok(vec![(width % 251) as u8; self.size]),
                _ => Err(PictureError::BadCode),
            }
        }
    }

    fn service(size: usize, limit: usize) -> (Service, Arc<AtomicUsize>, Arc<AtomicUsize>) {
        let (opened, drawn) = (Arc::new(AtomicUsize::new(0)), Arc::new(AtomicUsize::new(0)));
        let (o, d) = (opened.clone(), drawn.clone());
        let opener: Opener = Box::new(move || {
            o.fetch_add(1, Ordering::SeqCst);
            Ok(Box::new(Fake { drawn: d, size }) as Box<dyn Painter>)
        });
        (Service::with_limit(opener, limit), opened, drawn)
    }

    #[test]
    fn routes_are_a_size_and_an_encoded_code() {
        assert_eq!(parse_route("thumb/BL2(abc)"), Ok((Size::Thumb, "BL2(abc)".to_string())));
        // What the page's encodeURIComponent makes of a real code: + / = escaped, ( ) left.
        assert_eq!(
            parse_route("detail/BL2(hwAAAADX0ACE5wRACiGT6cOQhYEQowHG%2F38JGBkw%2Fv8rxACCgRED%2Bw%3D%3D)"),
            Ok((Size::Detail, "BL2(hwAAAADX0ACE5wRACiGT6cOQhYEQowHG/38JGBkw/v8rxACCgRED+w==)".to_string()))
        );
        assert_eq!(parse_route("detail/BL2%28a%29"), Ok((Size::Detail, "BL2(a)".to_string())));
        for bad in ["", "thumb", "huge/BL2(a)", "Thumb/BL2(a)", "/BL2(a)"] {
            let refusal = parse_route(bad).unwrap_err();
            assert_eq!((refusal.status, refusal.content_type), (400, "text/plain; charset=utf-8"), "{bad}");
        }
        // No code, a broken escape, bytes that are not text, an absurdly long code.
        let long = format!("thumb/{}", "A".repeat(MAX_CODE_LENGTH + 1));
        for bad in ["thumb/", "thumb/BL2%2", "thumb/BL2%zz", "thumb/%ff%fe", long.as_str()] {
            assert_eq!(parse_route(bad).unwrap_err(), Answer::refusal(&PictureError::BadCode), "{bad}");
        }
        assert_eq!((Size::Thumb.pixels(), Size::Detail.pixels()), ((288, 144), (528, 264)));
    }

    #[test]
    fn pictures_are_opened_lazily_drawn_once_and_then_served_from_memory() {
        let (mut service, opened, drawn) = service(100, 10_000);
        assert_eq!(opened.load(Ordering::SeqCst), 0);
        // A request that cannot be read does not open anything.
        assert_eq!(service.answer("nonsense").status, 400);
        assert_eq!(opened.load(Ordering::SeqCst), 0);

        let first = service.answer("detail/BL2(a)");
        assert_eq!((first.status, first.content_type, first.body.len()), (200, "image/png", 100));
        assert!(first.is_picture());
        assert_eq!(service.answer("detail/BL2(a)"), first);
        assert_eq!(service.answer("detail/BL2%28a%29"), first);
        assert_eq!((opened.load(Ordering::SeqCst), drawn.load(Ordering::SeqCst)), (1, 1));
        // The same code at another size is another picture.
        let thumb = service.answer("thumb/BL2(a)");
        assert_ne!(thumb.body, first.body);
        assert_eq!((drawn.load(Ordering::SeqCst), service.cached_bytes()), (2, 200));
    }

    #[test]
    fn the_cache_stays_within_its_bound_and_drops_the_least_recently_used() {
        let (mut service, _, drawn) = service(100, 350);
        for code in ["a", "b", "c"] {
            service.answer(&format!("thumb/BL2({code})"));
        }
        assert_eq!((service.cached_bytes(), drawn.load(Ordering::SeqCst)), (300, 3));
        // Use "a" again, then add a fourth: "b" is now the oldest and goes.
        service.answer("thumb/BL2(a)");
        service.answer("thumb/BL2(d)");
        assert_eq!((service.cached_bytes(), drawn.load(Ordering::SeqCst)), (300, 4));
        service.answer("thumb/BL2(a)");
        service.answer("thumb/BL2(c)");
        assert_eq!(drawn.load(Ordering::SeqCst), 4, "a and c were still held");
        service.answer("thumb/BL2(b)");
        assert_eq!(drawn.load(Ordering::SeqCst), 5, "b had been dropped");
        assert!(service.cached_bytes() <= 350);

        // A picture bigger than the whole bound is drawn and sent but not kept.
        let (mut small, _, drawn) = super::tests::service(100, 50);
        assert_eq!(small.answer("thumb/BL2(a)").status, 200);
        assert_eq!(small.answer("thumb/BL2(a)").status, 200);
        assert_eq!((small.cached_bytes(), drawn.load(Ordering::SeqCst)), (0, 2));
    }

    #[test]
    fn what_cannot_be_drawn_is_a_status_and_a_sentence() {
        let (mut service, _, _) = service(10, 1000);
        for (code, status, error) in [
            ("item", 404, PictureError::NotAWeapon),
            ("dlc", 404, PictureError::DlcItem),
            ("parts", 404, PictureError::UnknownParts),
            ("empty", 404, PictureError::NothingToDraw),
            ("broken", 503, PictureError::GameData("x".into())),
            ("hello", 400, PictureError::BadCode),
        ] {
            let answer = service.answer(&format!("detail/{code}"));
            assert_eq!((answer.status, answer.content_type), (status, "text/plain; charset=utf-8"), "{code}");
            assert_eq!(String::from_utf8(answer.body).unwrap(), error.message(), "{code}");
            assert!(!Answer::refusal(&error).is_picture());
        }
        // Refusals are not kept.
        assert_eq!(service.cached_bytes(), 0);
    }

    #[test]
    fn a_failed_open_is_remembered_and_answered_at_once() {
        let opened = Arc::new(AtomicUsize::new(0));
        let o = opened.clone();
        let mut service = Service::new(Box::new(move || {
            o.fetch_add(1, Ordering::SeqCst);
            std::thread::sleep(std::time::Duration::from_millis(60));
            Err(PictureError::GameData("no game".into()))
        }));
        let first = service.answer("detail/BL2(a)");
        assert_eq!(first.status, 503);
        assert_eq!(String::from_utf8(first.body.clone()).unwrap(), PictureError::GameData(String::new()).message());
        let started = std::time::Instant::now();
        for n in 0..50 {
            assert_eq!(service.answer(&format!("thumb/BL2({n})")), first);
        }
        assert!(started.elapsed() < std::time::Duration::from_millis(50), "{:?}", started.elapsed());
        assert_eq!(opened.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn the_detail_picture_jumps_the_queue() {
        assert_eq!(next_request::<&str>(&[]), None);
        assert_eq!(next_request(&["thumb/a", "thumb/b"]), Some(0));
        assert_eq!(next_request(&["thumb/a", "detail/b", "thumb/c", "detail/d", "thumb/e"]), Some(3));
    }

    /// Through the real game install: the fixtures' weapons come back as PNGs of the right
    /// size (shields, grenade mods and relics too), the rest as refusals. Skipped when the
    /// game is not installed.
    #[test]
    fn real_weapons_become_pngs() {
        if gamedata::GameInstall::locate().is_none() {
            eprintln!("skipped: no game install found");
            return;
        }
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/local");
        let mut service = Service::live();
        let (mut pictures, mut refusals, mut fixtures, mut item_pictures) = (0, 0, 0, 0);
        for name in ["win-throwaway-Save0001.sav", "mac-main-Save0001.sav"] {
            let Ok(bytes) = std::fs::read(dir.join(name)) else { continue };
            fixtures += 1;
            let doc = savekit::SaveDocument::from_file_bytes(&bytes).unwrap();
            let before = pictures;
            for entry in doc.inventory() {
                let Some(code) = entry.blob.as_ref().map(|blob| blob.code()) else { continue };
                let route = |size: &str| format!("{size}/{}", code.replace('/', "%2F").replace('+', "%2B").replace('=', "%3D"));
                for size in [Size::Detail, Size::Thumb] {
                    let answer = service.answer(&route(if size == Size::Detail { "detail" } else { "thumb" }));
                    // Weapons always have a picture; shields, grenade mods and relics do too,
                    // class mods and story items do not.
                    if entry.is_virtual || (!entry.is_weapon && answer.status == 404) {
                        assert_eq!(answer.status, 404, "{name} {}", entry.entry_ref);
                        refusals += 1;
                        continue;
                    }
                    if !entry.is_weapon {
                        item_pictures += 1;
                    }
                    assert_eq!((answer.status, answer.content_type), (200, "image/png"), "{name} {}: {}", entry.entry_ref, String::from_utf8_lossy(&answer.body));
                    let (width, height) = size.pixels();
                    assert_eq!(&answer.body[..8], &[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]);
                    assert_eq!((&answer.body[16..20], &answer.body[20..24]), (&width.to_be_bytes()[..], &height.to_be_bytes()[..]));
                    assert!(answer.body.len() > 2_000 && answer.body.len() < 120_000, "{} bytes", answer.body.len());
                    // Asked again, it is the same bytes, from memory.
                    assert_eq!(service.answer(&route(if size == Size::Detail { "detail" } else { "thumb" })), answer);
                    pictures += 1;
                }
            }
            println!("{name}: {} pictures", pictures - before);
        }
        if fixtures == 0 {
            eprintln!("skipped: no fixture saves");
            return;
        }
        println!("{pictures} pictures ({item_pictures} of items), {refusals} refusals, {} KiB held", service.cached_bytes() >> 10);
        assert!(pictures >= 6 && refusals > 0 && item_pictures > 0);
        assert!(service.cached_bytes() <= CACHE_BYTES);
    }
}

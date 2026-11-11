//! Developer tool: renders weapon pictures to PNG files, sweeps the part combinations the
//! game can produce, and times the steps. Reads the game install (read-only) and writes only
//! into the output folder given.
//!
//! ```text
//! gmrender <out-dir> [options]
//!   --save <file.sav>        every weapon in a save
//!   --code <BL2(...)>        one item code
//!   --build <spec>           "<weapon type path>;<body>;<grip>;<barrel>;<sight>;<stock>;
//!                             <elemental>;<accessory 1>;<accessory 2>;<material>" (part paths,
//!                             an empty field for an empty slot)
//!   --size <W>x<H>           picture size; may be given several times (default 1120x560)
//!   --backdrop <RRGGBB>      composite over this colour instead of leaving it transparent
//!   --facing left|right      which way the muzzle points (default left)
//!   --colouring plain|flat|textured
//!   --angle <yaw>,<pitch>    camera turn and rise in degrees (default 0,0)
//!   --crop <l>,<t>,<r>,<b>   keep only this part of each picture (fractions 0…1)
//!   --sheet <columns>        also one picture of all the subjects side by side (first size)
//!   --all <gestalt path>     every fragment of one gestalt mesh at once
//!   --sweep                  assemble and draw a sample of every balance's part combinations
//!   --bench                  timings
//! ```

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Instant;

use gamedata::package::UPackage;
use gamedata::{GameInstall, ItemRefs};
use gamemesh::balances::weapon_balances;
use gamemesh::picture::whole_gestalt_scene;
use gamemesh::render::{render, Image, View};
use gamemesh::{png, Assembly, Colouring, Facing, Pictures, Style, WeaponSource, WEAPON_SLOTS};
use savekit::{ItemFields, SaveDocument};

const INFINITY: &str = "BL2(hwAAAADX0ACE5wRACiGT6cOQhYEQowHG/38JGBkw/v8rxACCgRED)";

enum Subject {
    Item(String, ItemRefs),
    Built(String, Assembly),
}

struct Options {
    out: PathBuf,
    sizes: Vec<(u32, u32)>,
    backdrop: Option<[u8; 3]>,
    crop: Option<[f32; 4]>,
    style: Style,
}

fn refs(fields: &ItemFields, is_weapon: bool) -> ItemRefs {
    ItemRefs {
        is_weapon,
        set: fields.set,
        item_type: fields.item_type,
        balance: fields.balance,
        manufacturer: fields.manufacturer,
        parts: fields.parts,
    }
}

fn file_stem(name: &str) -> String {
    name.chars().map(|c| if c.is_ascii_alphanumeric() { c.to_ascii_lowercase() } else { '-' }).collect()
}

fn numbers<T: std::str::FromStr>(text: &str, separator: char, count: usize) -> Result<Vec<T>, String> {
    let values: Vec<T> = text.split(separator).filter_map(|v| v.trim().parse().ok()).collect();
    if values.len() == count {
        Ok(values)
    } else {
        Err(format!("expected {count} numbers separated by '{separator}', got {text:?}"))
    }
}

/// Lays `image` over a solid colour.
fn composite(image: &mut Image, backdrop: [u8; 3]) {
    for pixel in image.rgba.as_chunks_mut::<4>().0 {
        let alpha = u32::from(pixel[3]);
        for c in 0..3 {
            pixel[c] = ((u32::from(pixel[c]) * alpha + u32::from(backdrop[c]) * (255 - alpha) + 127) / 255) as u8;
        }
        pixel[3] = 255;
    }
}

fn crop(image: &Image, [l, t, r, b]: [f32; 4]) -> Image {
    let edge = |f: f32, size: u32| ((f.clamp(0.0, 1.0) * size as f32) as u32).min(size);
    let (x0, x1) = (edge(l, image.width), edge(r, image.width).max(edge(l, image.width) + 1).min(image.width));
    let (y0, y1) = (edge(t, image.height), edge(b, image.height).max(edge(t, image.height) + 1).min(image.height));
    let mut rgba = Vec::new();
    for y in y0..y1 {
        let row = (y * image.width) as usize * 4;
        rgba.extend_from_slice(&image.rgba[row + x0 as usize * 4..row + x1 as usize * 4]);
    }
    Image { width: x1 - x0, height: y1 - y0, rgba }
}

fn save(options: &Options, name: &str, mut image: Image) -> Result<usize, String> {
    if let Some(area) = options.crop {
        image = crop(&image, area);
    }
    // As `Pictures::weapon_png` does, so the files here are what the app would get.
    gamemesh::reduce_depth(&mut image.rgba, options.style.colour_bits);
    if let Some(colour) = options.backdrop {
        composite(&mut image, colour);
    }
    let file = png::encode_rgba(image.width, image.height, &image.rgba);
    let path = options.out.join(name);
    std::fs::write(&path, &file).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(file.len())
}

fn name_suffix(options: &Options) -> String {
    let mut suffix = String::new();
    if options.style.facing == Facing::MuzzleRight {
        suffix += "-right";
    }
    match options.style.colouring {
        Colouring::Plain => suffix += "-plain",
        Colouring::Flat => suffix += "-flat",
        Colouring::Textured => {}
    }
    if options.style.yaw != 0.0 || options.style.pitch != 0.0 {
        suffix += "-angled";
    }
    if options.crop.is_some() {
        suffix += "-crop";
    }
    if let Some([r, g, b]) = options.backdrop {
        suffix += &format!("-on-{r:02x}{g:02x}{b:02x}");
    }
    suffix
}

fn draw(pictures: &mut Pictures, options: &Options, subject: &Subject) -> Result<(), String> {
    let (label, assembly) = match subject {
        Subject::Item(label, item) => {
            let name = pictures.source().database().info(item).name.unwrap_or_else(|| label.clone());
            println!("{label}: {name}");
            match pictures.source().assemble(item) {
                Ok(assembly) => (name, assembly),
                Err(e) => {
                    println!("  cannot assemble: {e}");
                    return Ok(());
                }
            }
        }
        Subject::Built(label, assembly) => (label.clone(), assembly.clone()),
    };
    println!("  type {} -> {}", assembly.weapon_type, assembly.gestalt);
    for part in &assembly.parts {
        println!("  {:<12} {} -> {:?}", WEAPON_SLOTS[part.slot], part.definition, part.fragments);
    }
    println!("  material {:?}; unresolved slots {:?}", assembly.material, assembly.unresolved_slots);
    for &(width, height) in &options.sizes {
        let t = Instant::now();
        let image = match pictures.assembly_image(&assembly, width, height) {
            Ok(image) => image,
            Err(e) => {
                println!("  no picture: {e}");
                return Ok(());
            }
        };
        let drawn = t.elapsed();
        if let Some(problem) = pictures.skin_problem() {
            println!("  drawn without its skin: {problem}");
        }
        let name = format!("{}-{}x{}{}.png", file_stem(&label), image.width, image.height, name_suffix(options));
        let bytes = save(options, &name, image)?;
        println!("  {name}: drawn in {drawn:.1?}, {bytes} bytes");
    }
    Ok(())
}

/// All the subjects in one picture, `columns` to a row, over the backdrop (the app's page
/// colour unless another was given).
fn sheet(pictures: &mut Pictures, options: &Options, subjects: &[Subject], columns: u32) -> Result<(), String> {
    let (cell_w, cell_h) = options.sizes[0];
    let columns = columns.clamp(1, 12);
    let rows = (subjects.len() as u32).div_ceil(columns).max(1);
    let backdrop = options.backdrop.unwrap_or([27, 20, 13]);
    let mut sheet = Image { width: cell_w * columns, height: cell_h * rows, rgba: Vec::new() };
    sheet.rgba = [backdrop[0], backdrop[1], backdrop[2], 255].repeat((sheet.width * sheet.height) as usize);
    for (cell, subject) in subjects.iter().enumerate() {
        let assembly = match subject {
            Subject::Item(_, item) => pictures.source().assemble(item).ok(),
            Subject::Built(_, assembly) => Some(assembly.clone()),
        };
        let Some(mut image) = assembly.and_then(|a| pictures.assembly_image(&a, cell_w, cell_h).ok()) else { continue };
        gamemesh::reduce_depth(&mut image.rgba, options.style.colour_bits);
        composite(&mut image, backdrop);
        let (ox, oy) = ((cell as u32 % columns) * cell_w, (cell as u32 / columns) * cell_h);
        for y in 0..image.height.min(cell_h) {
            let from = (y * image.width) as usize * 4;
            let to = ((oy + y) * sheet.width + ox) as usize * 4;
            let len = image.width.min(cell_w) as usize * 4;
            sheet.rgba[to..to + len].copy_from_slice(&image.rgba[from..from + len]);
        }
    }
    let name = format!("sheet-{}x{}{}.png", sheet.width, sheet.height, name_suffix(options));
    let file = png::encode_rgba(sheet.width, sheet.height, &sheet.rgba);
    std::fs::write(options.out.join(&name), &file).map_err(|e| e.to_string())?;
    println!("{name}: {} subjects, {} bytes", subjects.len(), file.len());
    Ok(())
}

/// Per weapon class: how many combinations were tried and their sizes.
#[derive(Default)]
struct ClassStats {
    tried: usize,
    lengths: Vec<f32>,
    heights: Vec<f32>,
    coverage: Vec<f32>,
    samples: Vec<Assembly>,
}

/// Whether the parts of an assembled weapon touch: the number of groups of parts whose
/// bounding boxes (grown by `slack`) do not reach any other group.
fn detached_groups(source: &WeaponSource, assembly: &Assembly, slack: f32) -> usize {
    let Ok(gestalt) = source.gestalt(&assembly.gestalt) else { return 0 };
    let boxes: Vec<([f32; 3], [f32; 3])> = assembly
        .parts
        .iter()
        .filter_map(|part| {
            let indices: Vec<u32> = part
                .fragments
                .iter()
                .flat_map(|name| gestalt.fragments_named(name))
                .flat_map(|fragment| gestalt.indices_of(fragment).iter().copied())
                .collect();
            gestalt.mesh.bounding_box(&indices)
        })
        .collect();
    let touch = |a: &([f32; 3], [f32; 3]), b: &([f32; 3], [f32; 3])| (0..3).all(|k| a.0[k] - slack <= b.1[k] && b.0[k] - slack <= a.1[k]);
    let mut group: Vec<usize> = (0..boxes.len()).collect();
    for _ in 0..boxes.len() {
        for i in 0..boxes.len() {
            for j in 0..boxes.len() {
                if touch(&boxes[i], &boxes[j]) {
                    let low = group[i].min(group[j]);
                    (group[i], group[j]) = (low, low);
                }
            }
        }
    }
    group.sort_unstable();
    group.dedup();
    group.len()
}

fn sweep(pictures: &mut Pictures, options: &Options, startup: &Path) -> Result<(), String> {
    let package = UPackage::open(startup).map_err(|e| e.to_string())?;
    let balances = weapon_balances(&package);
    drop(package);
    let mut stats: BTreeMap<String, ClassStats> = BTreeMap::new();
    let mut failures: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let (mut balances_used, mut combinations, mut no_type) = (0, 0, 0);
    let mut parts_seen = std::collections::BTreeSet::new();
    pictures.set_style(Style { colouring: Colouring::Plain, ..options.style.clone() });
    let t = Instant::now();
    for balance in &balances {
        let Some(weapon_type) = balance.weapon_type.as_deref().filter(|t| pictures.source().weapon_types().any(|known| known == *t))
        else {
            no_type += 1;
            continue;
        };
        balances_used += 1;
        let rounds = balance.slots.iter().map(Vec::len).max().unwrap_or(0).max(1);
        for round in 0..rounds {
            let mut parts: [Option<&str>; 9] = [None; 9];
            for (slot, list) in balance.slots.iter().enumerate() {
                if !list.is_empty() {
                    parts[slot] = Some(list[round % list.len()].as_str());
                }
            }
            combinations += 1;
            let mut fail = |why: String| failures.entry(why).or_default().push(format!("{} round {round}", balance.balance));
            let assembly = match pictures.source().assemble_paths(weapon_type, &parts) {
                Ok(assembly) => assembly,
                Err(e) => {
                    fail(format!("cannot assemble: {e}"));
                    continue;
                }
            };
            if !assembly.unresolved_slots.is_empty() {
                let names: Vec<&str> = assembly.unresolved_slots.iter().filter_map(|&s| parts[s]).collect();
                fail(format!("part not defined in Startup.upk: {}", names.join(", ")));
                continue;
            }
            parts_seen.extend(assembly.parts.iter().map(|p| p.definition.clone()));
            let image = match pictures.assembly_image(&assembly, 160, 80) {
                Ok(image) => image,
                Err(e) => {
                    fail(format!("no picture: {e}"));
                    continue;
                }
            };
            let covered = image.rgba.as_chunks::<4>().0.iter().filter(|p| p[3] > 0).count() as f32 / (image.width * image.height) as f32;
            let gestalt = pictures.source().gestalt(&assembly.gestalt).map_err(|e| e.to_string())?;
            let indices: Vec<u32> = assembly
                .fragments()
                .flat_map(|(_, name)| gestalt.fragments_named(name))
                .flat_map(|fragment| gestalt.indices_of(fragment).iter().copied())
                .collect();
            let Some((lo, hi)) = gestalt.mesh.bounding_box(&indices) else {
                fail("empty bounding box".into());
                continue;
            };
            let detached = detached_groups(pictures.source(), &assembly, 2.0);
            if detached > 1 {
                fail(format!("{detached} groups of parts that do not touch ({})", assembly.gestalt));
            }
            if covered < 0.04 {
                fail(format!("covers only {:.1}% of the picture", covered * 100.0));
            }
            let class = stats.entry(assembly.gestalt.clone()).or_default();
            class.tried += 1;
            class.lengths.push(hi[1] - lo[1]);
            class.heights.push(hi[2] - lo[2]);
            class.coverage.push(covered);
            class.samples.push(assembly);
        }
    }
    println!(
        "sweep: {} balances ({balances_used} with a known weapon type, {no_type} without), {combinations} combinations, {} distinct parts, {:.1?}",
        balances.len(),
        parts_seen.len(),
        t.elapsed()
    );
    let spread = |values: &mut Vec<f32>| -> (f32, f32, f32) {
        values.sort_by(f32::total_cmp);
        (values[0], values[values.len() / 2], values[values.len() - 1])
    };
    for (class, s) in &mut stats {
        let (l, h, c) = (spread(&mut s.lengths), spread(&mut s.heights), spread(&mut s.coverage));
        println!(
            "  {class}: {} drawn; length {:.0}/{:.0}/{:.0}, height {:.0}/{:.0}/{:.0}, coverage {:.0}%/{:.0}%/{:.0}% (min/median/max)",
            s.tried,
            l.0,
            l.1,
            l.2,
            h.0,
            h.1,
            h.2,
            c.0 * 100.0,
            c.1 * 100.0,
            c.2 * 100.0
        );
    }
    println!("problems: {}", failures.values().map(Vec::len).sum::<usize>());
    for (why, cases) in &failures {
        println!("  {} x {why}; first: {}", cases.len(), cases[0]);
    }

    // A contact sheet per class: twelve combinations spread evenly over those tried.
    pictures.set_style(options.style.clone());
    let (cell_w, cell_h, columns, rows) = (280u32, 140u32, 4u32, 3u32);
    let backdrop = options.backdrop.unwrap_or([27, 20, 13]);
    for (class, s) in &stats {
        let mut sheet = Image { width: cell_w * columns, height: cell_h * rows, rgba: Vec::new() };
        sheet.rgba = [backdrop[0], backdrop[1], backdrop[2], 255].repeat((sheet.width * sheet.height) as usize);
        for cell in 0..columns * rows {
            let pick = cell as usize * s.samples.len() / (columns * rows) as usize;
            let Ok(mut image) = pictures.assembly_image(&s.samples[pick], cell_w, cell_h) else { continue };
            composite(&mut image, backdrop);
            let (ox, oy) = ((cell % columns) * cell_w, (cell / columns) * cell_h);
            for y in 0..image.height.min(cell_h) {
                let from = (y * image.width) as usize * 4;
                let to = ((oy + y) * sheet.width + ox) as usize * 4;
                let len = image.width.min(cell_w) as usize * 4;
                sheet.rgba[to..to + len].copy_from_slice(&image.rgba[from..from + len]);
            }
        }
        let name = format!("sweep-{}.png", file_stem(class.rsplit('.').next().unwrap_or(class)));
        let file = png::encode_rgba(sheet.width, sheet.height, &sheet.rgba);
        std::fs::write(options.out.join(&name), &file).map_err(|e| e.to_string())?;
        println!("  {name}: {} bytes", file.len());
    }
    Ok(())
}

/// Times opening and drawing. Writes nothing.
fn bench(install: &GameInstall) -> Result<(), String> {
    let t = Instant::now();
    let mut pictures = Pictures::open(install).map_err(|e| e.to_string())?;
    println!("open (read Startup.upk, tables, meshes, parts, materials): {:.0?}", t.elapsed());
    let item = Pictures::decode(INFINITY).map_err(|e| e.to_string())?;
    for (width, height) in [(1120, 560), (240, 120)] {
        for run in ["first (textures not in memory)", "again", "again"] {
            let t = Instant::now();
            let image = pictures.weapon_image(&item, width, height).map_err(|e| e.to_string())?;
            let drawn = t.elapsed();
            let t = Instant::now();
            let file = png::encode_rgba(image.width, image.height, &image.rgba);
            let encoded = t.elapsed();
            let smaller: Vec<String> = [7, 6, 5]
                .iter()
                .map(|&bits| {
                    let mut rgba = image.rgba.clone();
                    gamemesh::reduce_depth(&mut rgba, bits);
                    format!("{bits} bits {}", png::encode_rgba(image.width, image.height, &rgba).len())
                })
                .collect();
            println!("{width}x{height} {run}: draw {drawn:.0?}, PNG {encoded:.0?}, {} bytes ({})", file.len(), smaller.join(", "));
        }
    }
    for colouring in [Colouring::Flat, Colouring::Plain] {
        pictures.set_style(Style { colouring, ..Style::default() });
        for (width, height) in [(1120, 560), (240, 120)] {
            let t = Instant::now();
            let image = pictures.weapon_image(&item, width, height).map_err(|e| e.to_string())?;
            let drawn = t.elapsed();
            let file = png::encode_rgba(image.width, image.height, &image.rgba);
            println!("{width}x{height} {colouring:?}: draw {drawn:.0?}, {} bytes", file.len());
        }
    }
    println!("decoded textures held: {} MiB", pictures.texture_bytes() >> 20);
    Ok(())
}

fn run(args: &[String]) -> Result<(), String> {
    let usage = "usage: gmrender <out-dir> [--save f.sav] [--code BL2(...)] [--build spec] [--size WxH] [--backdrop RRGGBB] \
                 [--facing left|right] [--colouring plain|flat|textured] [--angle yaw,pitch] [--crop l,t,r,b] [--sheet columns] [--all gestalt] [--sweep] [--bench]";
    let out = PathBuf::from(args.first().ok_or(usage)?);
    let install = GameInstall::locate().ok_or("game install not found")?;
    if args.iter().any(|a| a == "--bench") {
        return bench(&install);
    }
    std::fs::create_dir_all(&out).map_err(|e| e.to_string())?;
    let t = Instant::now();
    let mut pictures = Pictures::open(&install).map_err(|e| e.to_string())?;
    println!("opened in {:.0?}; {} problems", t.elapsed(), pictures.source().problems().len());
    for problem in pictures.source().problems() {
        println!("  {problem}");
    }

    let mut options = Options { out, sizes: Vec::new(), backdrop: None, crop: None, style: Style::default() };
    let mut subjects = Vec::new();
    let mut wholes = Vec::new();
    let mut sweeping = false;
    let mut sheet_columns = None;
    let mut rest = args[1..].iter();
    while let Some(flag) = rest.next() {
        if flag == "--sweep" {
            sweeping = true;
            continue;
        }
        let value = rest.next().ok_or_else(|| format!("{flag} needs a value\n{usage}"))?;
        match flag.as_str() {
            "--save" => {
                let bytes = std::fs::read(value).map_err(|e| format!("{value}: {e}"))?;
                let doc = SaveDocument::from_file_bytes(&bytes).map_err(|e| e.to_string())?;
                for entry in doc.inventory() {
                    if let (Some(fields), false) = (&entry.fields, entry.is_virtual) {
                        subjects.push(Subject::Item(format!("save-{}", entry.entry_ref), refs(fields, entry.is_weapon)));
                    }
                }
            }
            "--code" => subjects.push(Subject::Item(format!("code-{}", subjects.len()), Pictures::decode(value).map_err(|e| e.to_string())?)),
            "--build" => {
                let mut fields = value.split(';');
                let weapon_type = fields.next().unwrap_or("");
                let mut parts: [Option<&str>; 9] = [None; 9];
                for (slot, path) in fields.enumerate().take(9) {
                    parts[slot] = Some(path).filter(|p| !p.is_empty());
                }
                let assembly = pictures.source().assemble_paths(weapon_type, &parts).map_err(|e| e.to_string())?;
                let label = format!("{}-{}", weapon_type.rsplit('.').next().unwrap_or("built"), subjects.len());
                subjects.push(Subject::Built(label, assembly));
            }
            "--size" => {
                let v = numbers::<u32>(value, 'x', 2)?;
                options.sizes.push((v[0], v[1]));
            }
            "--backdrop" => {
                let rgb = u32::from_str_radix(value, 16).map_err(|_| format!("bad colour {value}"))?;
                options.backdrop = Some([(rgb >> 16) as u8, (rgb >> 8) as u8, rgb as u8]);
            }
            "--facing" => options.style.facing = if value == "right" { Facing::MuzzleRight } else { Facing::MuzzleLeft },
            "--colouring" => {
                options.style.colouring = match value.as_str() {
                    "plain" => Colouring::Plain,
                    "flat" => Colouring::Flat,
                    _ => Colouring::Textured,
                }
            }
            "--angle" => {
                let v = numbers::<f32>(value, ',', 2)?;
                (options.style.yaw, options.style.pitch) = (v[0], v[1]);
            }
            "--crop" => {
                let v = numbers::<f32>(value, ',', 4)?;
                options.crop = Some([v[0], v[1], v[2], v[3]]);
            }
            "--sheet" => sheet_columns = Some(numbers::<u32>(value, ',', 1)?[0]),
            "--all" => wholes.push(value.clone()),
            other => return Err(format!("unknown option {other}\n{usage}")),
        }
    }
    if options.sizes.is_empty() {
        options.sizes.push((1120, 560));
    }
    pictures.set_style(options.style.clone());
    for subject in &subjects {
        draw(&mut pictures, &options, subject)?;
    }
    if let Some(columns) = sheet_columns {
        sheet(&mut pictures, &options, &subjects, columns)?;
    }
    for path in &wholes {
        let gestalt = pictures.source().gestalt(path).map_err(|e| e.to_string())?;
        let (width, height) = options.sizes[0];
        let image = render(&whole_gestalt_scene(gestalt), &View { width, height, rim: options.style.rim, ..View::default() });
        let name = format!("all-parts-{}{}.png", file_stem(path.rsplit('.').next().unwrap_or(path)), name_suffix(&options));
        println!("{path}: every fragment -> {name}, {} bytes", save(&options, &name, image)?);
    }
    if sweeping {
        sweep(&mut pictures, &options, &install.startup_package())?;
    }
    Ok(())
}

fn main() -> std::process::ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match run(&args) {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("gmrender: {message}");
            std::process::ExitCode::FAILURE
        }
    }
}

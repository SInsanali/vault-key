# Borderlands 2 save file format (PC/Mac), clean-room description

Status: derived from reading a reference editor (a Python BL2/TPS save editor) for this
document only. This document contains no code from it. Where the reference's behaviour is
odd or possibly wrong, that is called out. Items marked **UNSURE** are guesses or
unverified; a consolidated list is at the end.

Conventions:

- `u8/u16/u32` are unsigned integers; `i32` is a signed two's-complement 32-bit integer.
- **BE** = big-endian, **LE** = little-endian.
- "PC" means PC and Mac. The reference makes no distinction between them; Mac saves are
  assumed to be byte-identical in format (**UNSURE**: no Mac-specific handling exists in
  the reference, so nothing confirms or refutes this).
- "Console" means the reference's `--bigendian` mode (Xbox 360 / PS3 save payloads that
  have already been extracted from their console containers).
- CRC-32 everywhere means the standard zlib/PKZIP CRC-32: reflected polynomial
  `0xEDB88320`, initial value `0xFFFFFFFF`, final XOR `0xFFFFFFFF`, result taken as
  unsigned 32-bit.

Layers, outermost first:

```
file      = SHA-1 hash | u32 BE length | LZO1X stream
LZO out   = "WSG" blob: size | "WSG" | version | CRC-32 | length | Huffman bitstream | 4 zero bytes
Huffman   = serialized tree + codes  ->  protobuf message ("player")
protobuf  = top-level fields; some fields contain nested messages, packed arrays,
            a custom binary challenge blob, and obfuscated item/weapon blobs
```

---

## 1. Container

### 1.1 Outer file

| Offset | Size | Endian | Content |
|---|---|---|---|
| 0 | 20 | n/a | SHA-1 digest of bytes `[20, EOF)` (everything after the digest) |
| 20 | 4 | **BE always** | Length of the decompressed data, which is the full WSG blob (see 1.2) |
| 24 | rest | n/a | LZO1X compressed stream, ending with the LZO end-of-stream marker |

- The SHA-1 covers the 4-byte length field **and** the whole LZO stream, through EOF. No
  trailing bytes may follow it.
- On read, the reference checks the SHA-1 and rejects the file if it does not match.
- On read, the reference does **not** check the length field at offset 20 against the
  actual decompressed size. Writers must still set it exactly (see section 2).
- The length field is BE on both PC and console. The reference hard-codes this, and its own
  comments wonder whether that is right for consoles. It is correct for PC.
- The reference has a check for Xbox `CON ` STFS containers, but it compares bytes to a text
  string, so the check never fires. Console files have to be pulled out of their containers
  first. That has nothing to do with PC.

### 1.2 Inner "WSG" blob (the LZO output)

| Offset | Size | Endian | Field | Value / meaning |
|---|---|---|---|---|
| 0 | 4 | **BE always** | `size` | Number of bytes after this field: `15 + len(huffman_bytes)`, where `huffman_bytes` includes the 4 trailing zero bytes. Equals total WSG blob length − 4. |
| 4 | 3 | n/a | magic | ASCII `WSG` (`57 53 47`) |
| 7 | 4 | **platform** | `version` | `2`. PC: LE (`02 00 00 00`). Console: BE (`00 00 00 02`). |
| 11 | 4 | platform | `crc` | CRC-32 of the **decompressed protobuf bytes** (the Huffman *output*, not the Huffman stream) |
| 15 | 4 | platform | `length` | Length in bytes of the decompressed protobuf (number of symbols to Huffman-decode) |
| 19 | var | n/a | Huffman | Serialized Huffman tree, then the encoded symbols, packed as one bitstream padded with zero bits to a whole byte (section 3) |
| end−4 | 4 | n/a | padding | Four `00` bytes, always appended by the reference writer |

Reading, as the reference does it:

- It reads `version` as BE first. A value of `2` means a console file: CRC and length are
  read BE. A value of `0x02000000` means a PC file: CRC and length are read LE. Any other
  value is an error. So on read, byte order is **auto-detected** from the version field.
- It does **not** validate the magic, the `size` field, or the trailing zero bytes.
- After Huffman-decoding exactly `length` bytes, it checks that CRC-32 of the result equals
  `crc` and rejects the file if not.

Writing, as the reference does it:

- `size` is written BE regardless of platform.
- `version`, `crc` and `length` use the platform byte order: LE by default, BE with
  `--bigendian`.
- The 4 zero bytes are always appended after the byte-aligned Huffman stream.
  **UNSURE**: what they are for (probably read-ahead slack for the game's bit reader), but
  game-written files seem to have them and the reference always writes them. Keep them.

What each check covers:

| Check | Algorithm | Covers |
|---|---|---|
| File hash (offset 0) | SHA-1, 20 bytes, raw digest | File bytes `[20, EOF)`: BE length + LZO stream |
| WSG `crc` | CRC-32 | Decompressed protobuf bytes only |
| Item checksum (inside each item blob) | CRC-32 folded to 16 bits | See section 6.3 |

---

## 2. LZO1X layer

### 2.1 Variant and framing

- The algorithm is standard **LZO1X**. The reference compressor is a port of
  **LZO1X-1** (the miniLZO default compressor). Its decompressor is a port of the
  generic LZO1X decompressor, without bounds checking.
- The reference's framing works like this. Internally it uses a 5-byte header: one byte `0xF0`
  followed by the u32 BE uncompressed length. The file holds only the last 4 of those bytes
  (offset 20); the `0xF0` byte is dropped on write and re-added on read. In practice:
  **the file has a u32 BE uncompressed length at offset 20, and the raw LZO1X instruction
  stream starts at offset 24.** No other LZO header, checksum or block structure is
  present.
- The stream ends with the standard LZO1X end-of-stream marker, bytes `11 00 00`. This is
  an M4 match instruction with distance 0.

### 2.2 What a decompressor must handle

Implement a full, standard LZO1X decompressor. A port of `lzo1x_decompress_safe`
semantics is ideal, adding bounds checks against the input length and the declared output
length. All LZO1X compressor levels (1, 1_11, 1_12, 1_15, 999) emit streams that the one
LZO1X decoder reads. The game's own compressor is not identified by the reference
(**UNSURE**), so the decoder must not assume LZO1X-1-only instruction patterns.

Instruction set summary, for reference (standard LZO1X; 16-bit distances are LE):

| First byte `t` | State | Meaning |
|---|---|---|
| `t > 17` | very first instruction only | Literal run of `t − 17` bytes. If `t − 17 < 4`, the next instruction is decoded in "after short literal" state. Otherwise it is decoded in "after long literal run" state. |
| `0..15` | "expecting literal-run" state | Literal run of `t + 3` bytes. If `t == 0`, count the zero bytes that follow (n), then read one non-zero byte b: length = `n*255 + b + 15 + 3`. |
| `0..15` | "after long literal run" state | M1 3-byte match: distance = `1 + 0x0800 + (t >> 2) + (next << 2)`, length 3 |
| `0..15` | "after match or short literal" state | M1 2-byte match: distance = `1 + (t >> 2) + (next << 2)`, length 2 |
| `64..255` | match state | M2: length `(t >> 5) + 1`, distance `1 + ((t >> 2) & 7) + (next << 3)` |
| `32..63` | match state | M3: length `(t & 31) + 2`. If `t & 31 == 0`, length = `n*255 + b + 31 + 2` (zero-run extension as above). Then u16 LE `d`: distance `1 + (d >> 2)`, trailing-literal count `d & 3`. |
| `16..31` | match state | M4: high distance bit `(t & 8) << 11`, length `(t & 7) + 2` (zero-run extension with base 7 if `t & 7 == 0`). Then u16 LE `d`: distance `0x4000 + high + (d >> 2)`. If `high + (d >> 2) == 0`, this is **end of stream**. Trailing literals `d & 3`. |

After any match, the low 2 bits of the last instruction byte (`t & 3` for M1/M2, `d & 3`
for M3/M4) give 0–3 literal bytes to copy right away. If that count is 0, the next byte is
decoded in "expecting literal-run" state. Otherwise it is decoded in "after short literal"
state. Matches may overlap their own output (distance < length), so copy byte by byte.

Reference decoder quirk: after the *first* instruction of the stream is a literal run of
4 or more bytes, the reference decodes a following `t < 16` byte as a 2-byte M1 match.
Standard LZO1X decodes it as the 3-byte `0x0800`-based M1 match. LZO1X-1 never emits that
sequence, and it cannot happen in a literal-only stream, so it doesn't matter in practice.
Implement the standard behaviour.

### 2.3 Literal-run-only encoder: compatibility notes

A stream holding one literal run followed by EOS is valid LZO1X. The exact encoding for a
payload of `L` bytes (L ≥ 1):

- If `L ≤ 238`: one byte `17 + L`, then the L literal bytes.
- If `L ≥ 239`: the long form works for any `L ≥ 19`. Emit byte `00`. Let `tt = L − 18`.
  While `tt > 255`, emit `00` and subtract 255. Then emit `tt`, which is now in
  `1..255` and **must be non-zero**. Then emit the L literal bytes.
- Then emit EOS `11 00 00`.

Constraints and observations:

1. **Never emit two literal-run instructions back to back.** After a literal run of 4 or
   more bytes, a following byte `< 16` is a 3-byte M1 match, not another literal run. All
   literals must go in one run (or runs must be separated by matches). One run of
   arbitrary length is fine thanks to the zero-run length extension.
2. **Remainder byte must be non-zero.** The reference's own compressor computes the
   extension as a quotient and remainder by 255. When `tt` is an exact multiple of 255 it
   writes a final `00`, which the decoder would treat as one more extension byte, corrupting
   the stream. Use the "subtract 255 while > 255" rule above, which is the standard LZO
   rule.
3. The reference compressor works in 49152-byte input blocks with a fresh 16384-entry
   dictionary per block, like miniLZO. That limit is internal to its compressor. Nothing in
   the reference requires a decoder-side block size, a maximum literal-run length, or
   compressed output smaller than the input.
4. Output size is about `L + L/255 + 5` bytes. The payload is already Huffman-compressed
   and high-entropy, so a literal-only stream is only marginally larger than an LZO1X-1
   stream would be.
5. **UNSURE**: whether the game allocates a fixed-size buffer for the compressed file, or
   checks that the declared length (offset 20) equals the decompressed size. The
   reference gives no evidence either way. Always write the exact decompressed length.
6. Real saves are tens of kilobytes. That is a general estimate, not from the reference.
   **UNSURE**.

---

## 3. Huffman layer

### 3.1 Bit order

Everything in the WSG Huffman stream is **MSB-first**. Bit index `i` of the stream is bit
`7 − (i mod 8)` of byte `i / 8`, so bit 7 is the first bit of each byte. Multi-bit values
(tree leaf bytes, codes) are written most-significant bit first.

### 3.2 Tree serialisation

Pre-order and recursive, starting at stream bit 0, which is WSG offset 19:

- bit `1` → **leaf**: the next 8 bits (MSB-first) are the byte value.
- bit `0` → **internal node**: serialised left child (code bit `0`), then right
  child (code bit `1`).

There is no node count, length prefix or byte alignment. The symbol codes start at the
bit right after the last leaf byte. Every internal node has exactly two children.

### 3.3 Data

- Decode exactly `length` symbols (from the WSG header). For each one, start at the root,
  read one bit at a time (0 = left, 1 = right), and stop at a leaf.
- Bits after the last symbol (byte padding and the 4 trailing zero bytes) are ignored.

### 3.4 Encoding (how the reference builds the tree)

1. Count how often each byte value 0–255 appears in the protobuf payload. Keep the values
   whose count is non-zero.
2. Sort the leaves ascending by (count, byte value).
3. Repeat until one node is left: remove the first two nodes (A then B), make a new
   internal node with left = A, right = B and weight = sum, and insert it into the sorted
   list. Ordering rules: weight ascending. At equal weight, leaves sort before internal
   nodes and leaves sort by byte value. A new internal node goes **after** every
   existing node of the same weight.
4. Code for a symbol = path from the root (left = 0, right = 1).
5. Write the tree (3.2), then the code for each payload byte in order, then pad the final
   partial byte with **zero** bits in its low positions, then append `00 00 00 00`.

The exact tree shape is not semantically important: the decoder uses whatever tree is in
the file. Any full binary prefix tree should be accepted. Matching the tie-breaking above
only matters for byte-identical output with the reference.

Edge cases:

- A payload with **one** distinct byte value gives a tree that is a single leaf with
  0-bit codes, which cannot be decoded. An empty payload crashes the reference. Real saves
  never hit either case. If you must handle one, add a dummy second leaf.
- Code lengths are not limited. In theory a skewed distribution can produce codes longer
  than 32 bits. Don't hold codes in a `UInt32` without checking. **UNSURE**: whether the
  game's decoder has a maximum code length. It probably walks the tree like the reference.

---

## 4. Protobuf layer

### 4.1 Generic reader/writer (as the reference implements it)

The payload is a protobuf message without a schema. The reference parses it generically
and only interprets the fields it needs:

- Key = varint `(field_number << 3) | wire_type`.
- Supported wire types: **0** (varint), **1** (64-bit, LE), **2** (length-delimited:
  varint length + bytes), **5** (32-bit, LE). Wire types 3 and 4 (groups) are
  rejected. The game does not appear to use them.
- Varints: little-endian base-128, 7 bits per byte, high bit = continuation. They are
  read as unsigned values of any size. In Swift use `UInt64`: negative int32/int64
  values show up as 10-byte varints holding 64-bit two's complement, and they must
  round-trip exactly (see OP levels, 5.13).
- Parsed form: `field_number → ordered list of (wire_type, value)`. Repeated fields and
  repeated scalars keep their order. Wire-type-2 values stay as raw bytes until
  something explicitly parses them as a nested message, packed array, string or blob.
- **Unknown fields are kept**: anything the editor does not touch is written back with
  its original wire type and value bytes.
- Writer: fields are written **sorted by ascending field number**. Within a field, entries
  keep their original order. Varints are written in minimal form. Untouched nested
  messages are written back from their original bytes. Nested messages the editor
  modified are re-serialised, with the same field sorting, from the parsed form.
- A field given a *list* of integers with wire type 0 is written as a single **packed**
  wire-type-2 entry (concatenated varints). This is how the currency array is written
  back.

Recommendation for Swift: model each message as an ordered list of `(field, wireType,
payload)`. Only re-encode the submessages you change. Sorting top-level fields matches
the reference, and the game accepts that, but keeping the original order is the safest
choice.

### 4.2 Top-level fields

"Type" is the reference's interpretation. Wire types: `V` = 0 varint, `L` = 2
length-delimited, `F32` = 5. Field numbers missing from this table (5, 9, 10, 12, 14, 22,
27, 28, 31–33, 39, 40, 42, 44, 45, 47, 48, 50–52, …) are opaque to the reference and are
passed through untouched.

| # | Name (reference) | Wire | Repeated | Content / encoding | Touched by |
|---|---|---|---|---|---|
| 1 | class | L | no | String: character class asset path | — |
| 2 | level | V | no | Character level (1–80) | `--level`, `--itemlevels 0` (read) |
| 3 | experience | V | no | Total XP | `--level` |
| 4 | skill_points | V | no | Unspent skill points | — |
| 6 | currency | L | no | **Packed varints**: `[0]` money, `[1]` eridium, `[2]` seraph crystals, `[3]` not touched (**UNSURE** meaning), `[4]` torgue tokens | `--money/--eridium/--seraph/--torgue` |
| 7 | playthroughs_completed | V | no | 0 = NVHM only, 1 = TVHM unlocked, 2 = UVHM unlocked | `--unlock tvhm/uvhm`, `--oplevel`, `--copy-nvhm-missions` |
| 8 | skills | L | yes | Message: `1` name (string), `2` level (V), `3`, `4` unknown (V) | — |
| 11 | resources | L | yes | Message: `1` resource (string), `2` pool (string), `3` amount (**F32 float**, IEEE-754 single, LE), `4` level (V) | `--maxammo`, `--unlock ammo` |
| 13 | sizes | L | no | Message: `1` inventory = backpack slot count (V), `2` weapon_slots = equip slots open (V), `3` weapon_slots_shown (V, see 5.8) | `--backpack`, `--gunslots` |
| 15 | stats | L | no | **Custom binary challenge blob** (4.4) | `--challenges`, `--fix-challenge-overflow`, `--reset bad-touch`, `--diagnose-challenge-accepted` |
| 16 | active_fast_travel | L | yes | Strings | — |
| 17 | last_fast_travel | L | no | String | — |
| 18 | missions | L | yes | One message per playthrough, in order NVHM, TVHM, UVHM: `1` playthrough (V), `2` active mission (string), `3` mission data (repeated message: `1` name string, `2` status V, `3` is_from_dlc V, `4` dlc_id V, `5` bytes, `6` V, `7` bytes, `8`, `9`, `10` V, `11` level V) | `--copy-nvhm-missions`, `--reset doctors-orders` |
| 19 | appearance | L | no | Message: `1` name (string), `2`/`3`/`4` colours (each a message `1` a, `2` r, `3` g, `4` b, all V) | `--name` |
| 20 | save_game_id | V | no | Save slot id | `--save-game-id` |
| 21 | mission_number | V | no | — | — |
| 23 | unlocks | L | no | Byte array; each byte is an unlock id. `0x01` = Creature Slaughterdome. | `--unlock slaughterdome` |
| 24 | unlock_notifications | L | no | Byte array, same ids | `--unlock slaughterdome` |
| 25 | time_played | V | no | **UNSURE** units (probably seconds) | — |
| 26 | save_timestamp | as found | no | **UNSURE** (probably a string) | — |
| 29 | game_stages | L | yes | Message: `1` name, `2` level, `3` is_from_dlc, `4` dlc_id, `5` playthrough | — |
| 30 | areas | L | yes | Message: `1` name, `2` unknown | — |
| 34 | id | L | no | Message: `1`,`2`,`3`,`4` each **F32 (fixed32)**: a 128-bit identifier (**UNSURE**: probably a GUID) | — |
| 35 | wearing | L | yes | Strings (cosmetics worn) | — |
| 36 | black_market | L | no | **Packed varints**, SDU levels, index order: `0` rifle, `1` pistol, `2` launcher, `3` shotgun, `4` smg, `5` sniper, `6` grenade, `7` backpack, `8` bank | `--backpack`, `--bank`, `--unlock ammo`, `--maxammo` (read) |
| 37 | active_mission | V | no | — | — |
| 38 | challenges | L | yes | "Challenge unlocked/visible" records: message `1` name (string, challenge asset path), `2` is_from_dlc (V), `3` dlc_id (V) | `--unlock challenges` |
| 41 | bank | L | yes | Message: `1` item/weapon blob (L). Other subfields preserved. | export/import, JSON |
| 43 | lockouts | L | yes | Message: `1` name, `2` time, `3` is_from_dlc, `4` dlc_id | — |
| 46 | explored_areas | L | yes | Strings (UTF-8), map names such as `Ash_P` | `--print-unexplored-levels` |
| 49 | active_playthrough | V | no | — | — |
| 53 | items | L | yes | Message: `1` item blob (L), `2` "unknown2" (V: **quantity** for real items, payload for virtual items, 5.13), `3` is_equipped (V, 0/1), `4` mark ("star") (V) | `--itemlevels`, `--oplevel`, export, import |
| 54 | weapons | L | yes | Message: `1` weapon blob (L), `2` quick slot (V: 0 = backpack, 1–4 = equipped slot), `3` mark ("star") (V), `4` unknown (V) | `--itemlevels`, export, import |
| 55 | stats_bonuses_disabled | V | no | Badass-rank bonuses toggle | — |
| 56 | bank_size | V | no | Bank slot count (absolute) | `--bank` |

Notes:

- "Mark" values: the reference only ever writes `1` on import. **UNSURE**: from general
  community knowledge, 0 = trash, 1 = standard, 2 = favourite.
- Strings are length-delimited bytes. The reference *writes* strings it sets (the name,
  challenge names, ammo resource names) as **Latin-1**, and reads explored-area names as
  UTF-8. **UNSURE**: the game's actual string encoding. Plain ASCII is safe.
- Field 6 has to have at least 5 entries for the reference's currency setter to work.
  Field 36 must have exactly 9 entries for `--maxammo` (it asserts this). `--bank` pads
  field 36 with zeros up to 9 entries.

### 4.3 JSON modes (reference-only convenience, not a file format)

- `-o decoded`: writes the raw decompressed protobuf bytes.
- `-o decodedjson`: writes the generic parsed form as JSON, keyed by field number
  (strings). Values are `[wire_type, value]` pairs. Byte values are written as
  Latin-1-decoded strings.
- `-o json`: maps known fields to the names in 4.2 (nested messages become objects, packed
  arrays become lists, floats are decoded, field 15 becomes a challenge object, field 36
  becomes a name → level object, and item blobs are decoded as in section 6). Unknown
  fields go under a `_raw` key per message as `{field: [[wire, value], ...]}`, with
  wire-2 values as lists of byte ints.
- `--json` (input): if the top-level JSON object has a key `"1"` it is the
  `decodedjson` form; otherwise it is the named form. Either way it is turned back into a
  protobuf and wrapped. Known loss: a non-repeated named field that appears more than
  once in the protobuf keeps only its first occurrence. The output byte order follows
  `--bigendian`.

### 4.4 Field 15: challenge blob (custom binary, NOT protobuf)

All multi-byte integers use **platform byte order** (LE on PC, BE with `--bigendian`).
There is no padding or alignment.

Header (10 bytes):

| Offset | Size | Field | Notes |
|---|---|---|---|
| 0 | u32 | unknown | Observed value 4; preserve it |
| 4 | u32 | size_in_bytes | `= 12 * count + 2`; also `= total_blob_length − 8` |
| 8 | u16 | count | Number of records |

Then `count` records of 12 bytes each:

| Offset | Size | Field | Notes |
|---|---|---|---|
| 0 | u16 | id | Challenge identifier, the key into the challenge table (section 7) |
| 2 | u8 | first_one | Observed value 1; preserve it |
| 3 | u32 | total_value | Running total, *not* reset by in-game challenge resets |
| 7 | u8 | second_one | Observed value 1; preserve it |
| 8 | u32 | previous_value | Value at the last in-game reset (0 if never reset) |

- Current displayed progress = `total_value − previous_value`.
- On read, the reference checks `size_in_bytes + 8 == blob length` and
  `count * 12 == size_in_bytes − 2`.
- On write it recomputes `size_in_bytes` and `count` from the record list and keeps the
  record order. The game keys records by `id`, not by position (the reference author's
  observation).
- The reference's own comment says the 2-byte id might really be two separate bytes (an
  id-ish byte, then a byte that is 6 or 7). Reading it as one u16 in platform order gives
  a unique id. **UNSURE** what the bytes mean individually.

---

## 5. CLI options: exact effects

Order of operations within one run: (JSON input conversion) → `--import-items` →
modifications in this order: level, currency, item levels, OP level, backpack, bank,
gun slots, copy-NVHM-missions, unlocks (slaughterdome, tvhm/uvhm, challenges, ammo),
max ammo, challenge levels, challenge overflow fix, name, save-game-id, `--reset`
→ re-wrap → read-only reports (run against the *output* data) → output. Every run
re-wraps the save even if nothing changed: a new Huffman tree, a new LZO stream, and
top-level fields sorted.

### 5.1 `--name NAME`
Parses field 19 and sets its subfield 1 to NAME (written as Latin-1 bytes). Colours and
unknown subfields are preserved, and field 19 is re-serialised with sorted subfields.
The name must be non-empty.

### 5.2 `--save-game-id N`
Sets top-level field 20 to N (a positive integer, varint).

### 5.3 `--level L` (1–80)
- Sets field 2 to L. The field is replaced by a single varint entry.
- XP (field 3): let `T[k]` be the minimum total XP for level k (table in section 7, 80
  entries, `T[1] = 0`).
  - If L = 80: set XP to exactly `T[80]` if it differs.
  - Otherwise, if XP < `T[L]` or XP ≥ `T[L+1]`, set XP = `T[L]`. If XP is already in
    range, leave it.
- Skill points (field 4) are **not** adjusted.

### 5.4 `--money`, `--eridium`, `--seraph`, `--torgue`
Decodes field 6 as packed varints and overwrites by index: money → `[0]`, eridium →
`[1]`, seraph → `[2]`, torgue → `[4]`. Index `[3]` is untouched. The result is written
back as one packed wire-2 entry. There is no range check. **UNSURE**: the game's
caps. The reference doesn't check them, but the values are probably signed 32-bit in-game,
so keep them ≤ 2^31−1 and never negative (the reference's varint writer cannot encode
negatives).

### 5.5 `--itemlevels N` and `--forceitemlevels`
- Target level = N if N > 0, otherwise the character level from field 2, read **after**
  `--level` has been applied.
- Applies to every entry of field **53 and 54** (not to the bank, field 41). For each
  entry: parse the submessage, decode the blob in subfield 1 (section 6), and look at
  `grade_index` (value [4]):
  - If it is absent (blob too short), skip the entry.
  - If `--forceitemlevels` is set, or grade_index > 1: set **both** `grade_index` ([4])
    and `game_stage` ([5]) to the target level. Then re-encode with the **same key**
    (checksum recomputed) and re-serialise the submessage.
  - Otherwise (level 0 or 1, not forced), leave the entry alone.
- Width is 7 bits each, so the maximum is 127. The reference does not mask the value
  (see Gotchas).
- Reference bug: with `--forceitemlevels`, the *virtual* items (5.13: set = 255, all
  other values 0, grade_index 0) also get their level fields set. **Skip virtual items.**

### 5.6 `--backpack N|max`
- N is clamped to [12, 39]; `max` = 39.
- `sdu = ceil((N − 12) / 3)`, `size = 12 + 3 * sdu`.
- Field 13 subfield 1 = `size` (**absolute slot count**).
- Field 36 index 7 = `sdu` (**SDU count**). The other indices are preserved.

### 5.7 `--bank N|max`
- N is clamped to [6, 24]; `max` = 24.
- `sdu = min(255, ceil((N − 6) / 2))`, `size = 6 + 2 * sdu`.
- Field 56 = `size` (absolute; created if missing).
- Field 36: padded with zeros to at least 9 entries, then index 8 = `sdu`.

### 5.8 `--gunslots 2|3|4`
Field 13 subfield 2 = n. If field 13 subfield 3 > n − 2, it is lowered to n − 2.
**UNSURE**: what subfield 3 means. It looks like "extra weapon slots beyond the first
two that the UI shows".

### 5.9 `--unlock tvhm` / `--unlock uvhm`
Field 7 = max(field 7, 1) for tvhm, or max(field 7, 2) for uvhm. uvhm wins if both are
given. Field 7 must already exist.

### 5.10 `--unlock slaughterdome`
Fields 23 and 24 are byte arrays (created empty if missing). If byte `0x01` is not
present in one, `0x01` is appended to it. Each field is written as a single wire-2 entry.

### 5.11 `--unlock challenges`
- Collects the `name` (subfield 1) of every existing field-38 entry.
- For each challenge in the reference challenge table (section 7), sorted
  case-insensitively by its asset path (`id_text`), whose path is not already present:
  append a new field-38 entry `{1: id_text (string), 2: is_from_dlc (0/1), 3: dlc_id}`.
  Subfields are written in that order.
- DLC ids used: Pirate's Booty 1, Campaign of Carnage 3, Hammerlock's Hunt 4, Dragon Keep
  (Tiny Tina) 9. Base-game challenges: is_from_dlc 0, dlc_id 0.
- This needs the challenge table's asset paths and DLC categories.

### 5.12 `--unlock ammo` and `--maxammo`
- `--unlock ammo` sets field 36 indices 0–6 (the seven ammo types) to **7**. Backpack
  and bank are untouched. It also turns on `--maxammo`.
- `--maxammo`:
  1. Read field 36 (must have exactly 9 entries). For each ammo type,
     `lvl = min(sdu_level, 7)`, `max = base + step * lvl`:

     | Type | base | step | Resource string (field 11.1) | Pool string (field 11.2) |
     |---|---|---|---|---|
     | grenade | 3 | 1 | `D_Resources.AmmoResources.Ammo_Grenade_Protean` | `D_Resourcepools.AmmoPools.Ammo_Grenade_Protean_Pool` |
     | launcher | 12 | 3 | `D_Resources.AmmoResources.Ammo_Rocket_Launcher` | `D_Resourcepools.AmmoPools.Ammo_Rocket_Launcher_Pool` |
     | pistol | 200 | 100 | `D_Resources.AmmoResources.Ammo_Repeater_Pistol` | `D_Resourcepools.AmmoPools.Ammo_Repeater_Pistol_Pool` |
     | rifle | 280 | 140 | `D_Resources.AmmoResources.Ammo_Combat_Rifle` | `D_Resourcepools.AmmoPools.Ammo_Combat_Rifle_Pool` |
     | shotgun | 80 | 20 | `D_Resources.AmmoResources.Ammo_Combat_Shotgun` | `D_Resourcepools.AmmoPools.Ammo_Combat_Shotgun_Pool` |
     | smg | 360 | 180 | `D_Resources.AmmoResources.Ammo_Patrol_SMG` | `D_Resourcepools.AmmoPools.Ammo_Patrol_SMG_Pool` |
     | sniper | 48 | 12 | `D_Resources.AmmoResources.Ammo_Sniper_Rifle` | `D_Resourcepools.AmmoPools.Ammo_Sniper_Rifle_Pool` |

  2. For each field-11 entry whose resource string matches a row: set subfield 4
     (level) = `lvl` and subfield 3 (amount) = `max` as a float32 (fixed32 LE bits).
     Re-serialise with sorted subfields; unknown subfields are kept. Unmatched
     resources only produce an error message.
  3. For each ammo type with no field-11 entry, append a new entry
     `{1: resource, 2: pool, 3: float amount, 4: level}`.

### 5.13 `--oplevel N` (0–10)
OP level lives in a "virtual item" in field 53.

- **Virtual item**: a field-53 entry whose blob decodes (section 6) to set = 255 and every
  other value = 0. Its subfield 2 holds a 64-bit varint which, read as signed int64, is
  `−(id | (value << 8))`. To decode: negate the int64; `id = low 8 bits`,
  `value = remaining bits >> 8`. ids, according to the reference's comment (community knowledge):
  1 = currency on hand, 2 = last playthrough / playthroughs completed, 3 = has played
  UVHM, 4 = **OP levels unlocked**, 5 = last OP selection. **UNSURE** for every id
  except 4 (only id 4 is used in code).
- Write: `v = −(4 | (clamp(N, 0, 0x7FFFFF) << 8))` as int64, stored as its uint64 two's
  complement in a (10-byte) varint.
- Find the first field-53 entry that has subfield 2, is a virtual item, and has id 4.
  Replace its subfield 2 with v and re-serialise the entry.
- If none is found, append a new field-53 entry `{1: B, 2: v, 3: 0, 4: 0}`. B is this
  fixed 40-byte blob (key 0, so not obfuscated):

  `07 00 00 00 00 | 39 2A | FF 00×32`

  That is: header byte 0x07 (item, version 7), key 0, checksum 0x392A (BE), and 33 item
  bytes (set = 0xFF, all else zero). The checksum is reproducible with 6.3 (verified
  independently).
- If N > 0 and field 7 < 2, UVHM is also unlocked (field 7 = 2).

### 5.14 `--copy-nvhm-missions`
Replaces the raw payloads of the 2nd and 3rd field-18 entries with the payload of the 1st
entry (a byte-for-byte copy). Field 18 needs at least 3 entries. It also unlocks UVHM
(field 7 ≥ 2). Caveat: the copied payload includes subfield 1 (playthrough) = 0, so all
three entries say playthrough 0. **UNSURE** whether the game uses position or subfield
1. Safer re-implementation: copy, then set subfield 1 to 1 and 2 respectively.

### 5.15 `--challenges zero|max|bonus` (repeatable)
Decode field 15 (4.4). For each record whose id is in the challenge table (records not in
the table are untouched), let `p = previous_value` and `levels` = the table thresholds:
- zero: `total = p`
- max: `total = p + levels[last] − 1`
- bonus (only challenges that have a bonus level index `b`, 1-based):
  `bv = p + levels[b] − 1`. Set `total = bv` if zero or max was also given, or if
  `total < bv`.

The steps are applied in the order zero → max → bonus, then the blob is re-encoded.

### 5.16 `--fix-challenge-overflow`
For each record in the table with `total_value ≥ 2,000,000,000`, set
`total_value = levels[last]`. `previous_value` is ignored.

### 5.17 `--reset bad-touch | doctors-orders`
- `bad-touch`: in field 15, set `total_value = 0` for the record with id **1836**. This
  does not need the table.
- `doctors-orders`: go through the field-18 entries from last to first (UVHM → TVHM →
  NVHM). In each one, parse it, and if it has subfield 3, drop every subfield-3 entry
  whose raw bytes contain the ASCII substring `GD_Z2_DoctorsOrders.M_DoctorsOrders`. Then
  re-serialise. Stop after the first playthrough where something was removed.
  (Playthroughs visited before that are re-serialised with sorted fields but otherwise
  unchanged.)

### 5.18 `--print-unexplored-levels` (read-only)
Reads field 46 (UTF-8 map names). For each explorer achievement in the
explorer-achievement table (section 7), it prints which required map names are missing.

### 5.19 `--diagnose-challenge-accepted` (read-only)
For field-15 records that are in the table and belong to a base-game (non-DLC) category:
it reports `total_value > 2,147,483,647` (overflow; suggests the fix option) or
`total_value < levels[0]` (first tier incomplete). Note that it uses `total_value`, not
`total − previous`.

### 5.20 `-o items` (export)
For fields 41 ("Bank"), 53 ("Items") and 54 ("Weapons"), in that order, and only when the
field is present: write a line `; Bank` / `; Items` / `; Weapons`. Then, for each entry,
take subfield 1 (the blob). Skip virtual items. Otherwise re-key the blob to key 0 (6.5)
and write `BL2(<base64>)`, one per line.

### 5.21 `--import-items FILE`
- Line by line, with whitespace trimmed: a line starting with `;` sets the mode. If the
  rest (trimmed, lowercased) is `bank`, mode is bank; `items` or `weapons` → inventory;
  anything else leaves the mode unchanged. Lines that do not start with `BL2(` and end with
  `)` are ignored. Text whose base64 does not decode is skipped. (The reference uses a
  lenient decoder that drops non-alphabet characters.)
- Each blob is re-keyed to a random key in [−2^31, 2^31−1] (6.5).
- Destination and default submessage:

  | Case | Field | Submessage |
  |---|---|---|
  | bank mode | 41 | `{1: blob}` |
  | inventory, header bit 7 = 0 (item) | 53 | `{1: blob, 2: 1 (quantity), 3: 0 (not equipped), 4: 1 (mark)}` |
  | inventory, header bit 7 = 1 (weapon) | 54 | `{1: blob, 2: 0 (no quick slot), 3: 1 (mark)}` (no subfield 4) |

- Entries are appended to the end of the repeated field, which is created if missing.
  There is no check against backpack or bank capacity, and the version byte and checksum
  are not checked either. **UNSURE** how the game treats overfull inventories.

### 5.22 `-o json` / `--json`
See 4.3.

### 5.23 `--bigendian`
Switches the WSG `version/crc/length` fields and the field-15 challenge blob to BE. On
read, WSG byte order is auto-detected, but the challenge blob always follows the flag.
That means reading a console save without `-b` succeeds until field 15 is decoded. Item
blobs and the outer length are unaffected, since they are BE everywhere.

---

## 6. Item and weapon blobs

### 6.1 Where they live

| Location | Blob field | Other subfields |
|---|---|---|
| Field 41 (bank), each entry | subfield 1 | (preserve any others) |
| Field 53 (items: shields, grenades, class mods, relics, etc.; plus virtual items) | subfield 1 | 2 quantity (or virtual payload), 3 is_equipped, 4 mark |
| Field 54 (weapons) | subfield 1 | 2 quick slot (0 = unequipped, 1–4), 3 mark, 4 unknown |

The bank can hold both weapons and items; tell them apart with the header bit.

### 6.2 Raw blob layout

| Offset | Size | Content |
|---|---|---|
| 0 | 1 | Header byte: bit 7 = **is_weapon**; bits 0–6 = **item struct version** (BL2 = **7**; the Pre-Sequel uses 10) |
| 1 | 4 | **key**, i32 **BE on every platform** |
| 5 | N+2 | Obfuscated body (encrypts `checksum(2) ‖ item(N)`) |

Observed sizes: weapons are usually 32 item bytes (39 total); items vary (25 bytes in a sample).

### 6.3 Checksum

```
padded = item ‖ (0xFF × max(0, 33 − N))     // pad item bytes to 33 with 0xFF
crc    = CRC-32( header[0..4] ‖ FF FF ‖ padded )   // header = byte0 + key BE (5 bytes)
chk16  = (crc >> 16) XOR (crc & 0xFFFF)
```

`chk16` is stored as **u16 BE** at the start of the plaintext body. In the CRC input, the
`FF FF` sits where the checksum will go. The checksum covers the header byte and the key,
so **changing the key or any item bit requires recomputing it**. The reference never
verifies checksums on read. **UNSURE**: the game almost certainly does, and treats
mismatches as invalid items.

### 6.4 Obfuscation

Let `key` be the signed i32 and `plain = chk16(BE) ‖ item`, length M = N+2.

- `rot = (key & 31) mod M`, using the low 5 bits of the two's-complement value.
- `seed = (key >> 5)` as an **arithmetic** (sign-extending) shift on the signed i32, then
  reinterpreted as u32.
- **Encrypt**: rotate `plain` **left** by `rot` bytes (`out[i] = plain[(i + rot) mod M]`),
  then XOR with the keystream.
- **Decrypt**: XOR with the keystream, then rotate **right** by `rot` bytes. Bytes 0–1
  are the checksum and bytes 2… are the item bitstream.
- Keystream: `s = seed`. For each byte position k = 0, 1, 2, …:
  `s = (s × 279470273) mod 4294967291`, then `byte[k] ^= (s & 0xFF)`.
  The modulus is the prime 2^32−5, **not** 2^32. Do the multiply in 64-bit (the product
  is < 2^62). The keystream is applied to the rotated buffer, by position.
- Key 0 gives rotation 0 and seed 0, so the keystream is all zeros: **the body is
  plaintext**.

### 6.5 Re-keying (export / import)

Decrypt with the old key and drop the 2 checksum bytes to get the item bytes. Build a new
header: same byte 0, new key as i32 BE. Recompute the checksum over the new header (6.3).
Encrypt with the new key. Export uses key 0; import uses a random i32.

Item code string: `BL2(` + standard Base64 (alphabet `A–Z a–z 0–9 + /`, with `=`
padding) of the **whole re-keyed blob** (header + body) + `)`. (The Pre-Sequel uses the
prefix `BLOZ`.)

Test vectors. These were checked with an independent implementation of this section; each
round-trips back to the original exactly:

| Original (from the reference README) | Re-keyed to key 0 |
|---|---|
| `BL2(h0Hd1Z+jY/s2Qy++Zu8Ba9qXoOmjwJ6NhrlsOmhNMX+oJo5CfQns)`: weapon, v7, key 1105057183, 32 item bytes | `BL2(hwAAAADX0ACE5wRACiGT6cOQhYEQowHG/38JGBkw/v8rxACCgRED)` |
| `BL2(B2vuv4tz1zSQCf2pqLJCS5XD/tKN4FXpjRJLnn1v85U=)`: item, v7, key 1810808715, 25 item bytes | `BL2(BwAAAABrxwAYC17YIBGBAABAAMD/////////Pw2AAwA=)` |

Decoded values (section 6.6 order): weapon = set 0, type 1924, balance 131111,
manufacturer 133, grade 50, stage 50, then 11 parts (some = 131071, i.e. all ones).
Item = set 0, type 2840, balance 27695, manufacturer 137, grade 1, stage 1, parts
`0, 1, 65535, 65535, 65535, 65535, 52, 14`, then 3 parts absent.

### 6.6 Item bitstream (after decryption, item bytes only)

**Bit order: LSB-first.** Value bit j of a field starting at stream bit position p goes to
stream bit `p + j`. Stream bit q is bit `q mod 8` (counting from the LSB) of byte
`q / 8`. In other words, the item bytes are one little-endian integer, and fields are
consecutive bit ranges starting at bit 0. (This is the opposite of the Huffman layer.)

Field widths, in order:

| # | Field | Item width | Weapon width | Split into (lib, asset): asset = low bits |
|---|---|---|---|---|
| 0 | set | 8 | 8 | none (255 marks a virtual item) |
| 1 | type | 17 | 13 | item: lib 9 / asset 8; weapon: lib 7 / asset 6 |
| 2 | balance | 20 | 20 | lib 10 / asset 10 |
| 3 | manufacturer | 11 | 11 | lib 4 / asset 7 |
| 4 | grade_index (item level) | 7 | 7 | none |
| 5 | game_stage | 7 | 7 | none |
| 6–16 | 11 part slots | 16 each | 17 each | item: lib 6 / asset 10; weapon: lib 6 / asset 11 |

Totals when every field is present: item 246 bits (31 bytes), weapon 253 bits (32 bytes).

`lib = value >> asset_bits` and `asset = value & ((1 << asset_bits) − 1)`. These are
indices into the game's asset libraries (package/sub-library index and asset index).
The reference does **not** resolve them to names.

Part slot names are **UNSURE**: they are not in the reference. From community knowledge
(community documentation), weapon slots are body, grip, barrel, sight, stock, elemental, accessory1,
accessory2, material, prefix, title, and item slots are alpha, beta, gamma, delta,
epsilon, zeta, eta, theta, material, prefix, title. In the samples, the all-ones value
(65535 / 131071) shows up where a part is empty. **UNSURE** that this means "none".

Variable length:

- A blob may end early. On read, a field whose end bit would go past `8 × N` is
  **absent**. Treat every later field as absent too. (The reference does not advance its
  bit position past an absent field, so in a pathologically short blob it could read a
  later, narrower field from that position. Don't copy that.) Absent part slots
  (and absent level fields, for very short blobs) are normal.
- On write, fields are packed in order up to the first absent one, and the length is
  `ceil(bits / 8)`. The reference sets the unused high bits of the final byte to **1**.
  Both externally produced samples above have those bits as **0**. Either is
  self-consistent, since the checksum covers them. **UNSURE** whether the game cares.
  To minimise differences, keep the original trailing bits when re-encoding an
  existing item. For new items, use 0, as the game samples do.
- Item level as displayed in game = `grade_index`. `--itemlevels` sets both
  `grade_index` and `game_stage`.

---

## 7. Data tables in the reference

| Table | Size / shape | Used for | Needed to show item level / manufacturer / balance names? |
|---|---|---|---|
| Required XP per level | 80 integers, levels 1–80 (level 1 = 0). This is the minimum total XP for each level. The reference hard-codes it because the public formula `ceil(60·L^2.8 − 60)` drifts by up to ±3 in double precision. An independent check found that evaluating it in emulated float32 still misses 13 of the 80 levels by ±1. **So the exact table is required**: source it from game data or a public wiki table, and do not compute it. | `--level` | No |
| Black-market ammo maxima | 7 ammo types × 8 levels (arithmetic progressions; base/step given in 5.12) | `--maxammo`, `--unlock ammo` | No |
| Ammo resource/pool names | 8 pairs (7 in BL2 plus a Pre-Sequel laser pair); BL2 ones listed in 5.12 | `--maxammo` | No |
| Black-market key order | 9 names (index order in 4.2, field 36) | backpack/bank/ammo SDUs | No |
| Challenge table (`bl2_data`) | **144** non-level-specific challenges, keyed by the u16 id from field 15. Each has: asset path (`id_text`, e.g. a `GD_Challenges.…` path), category (21 categories: 17 base-game ones flagged "counts for Challenge Accepted", plus 4 DLC ones with dlc ids 1/3/4/9), display name, description, tier thresholds (5 tiers for 143 of them, 4 for one), an optional 1-based bonus tier index (33 challenges have one: tiers 3, 4 or 5), and an unused "position". | `--challenges`, `--fix-challenge-overflow`, `--unlock challenges`, `--diagnose-challenge-accepted`, JSON annotations | No |
| Explorer achievements | 7 achievements, each a map of map-code (e.g. `Ash_P`) → display name, plus a guide URL. There is also an unused map of other map names. | `--print-unexplored-levels` | No |

**The reference has no asset/part/balance name tables.** Item level comes straight from
the blob (`grade_index`), so no table is needed for it. Showing **manufacturer, balance
(item name/rarity), type or part names** needs an extra dataset that maps
`(lib, asset)` indices to object names. In other editors this comes from the game's
asset library manifest, organised per set id and per category. It has to come from a
source other than this reference. **UNSURE**: the exact structure of that manifest; it
is out of scope here.

---

## 8. Gotchas (ways to produce a save the game rejects)

1. **SHA-1 scope**: hash bytes 20…EOF, which includes the 4-byte BE length. Write the hash
   last.
2. **Outer length** at offset 20 is BE and must equal the decompressed WSG size exactly.
3. **LZO literal runs**: never write two literal-run instructions back to back. A long run's
   extension remainder byte must be non-zero (the reference compressor gets this wrong at
   multiples of 255). End with `11 00 00`.
4. **WSG fields on PC**: version, CRC and length are **LE**; the `size` field at offset 0 is
   **BE**; `size = WSG total − 4`. Keep the 4 trailing zero bytes.
5. **WSG CRC** is over the protobuf payload, not the Huffman stream.
6. **Huffman**: MSB-first bits, leaf flag `1`, internal flag `0`, left = 0. Pad the final
   byte with zeros. Handle more than 32-bit code lengths. A payload with one distinct
   byte value can't be encoded.
7. **Protobuf fidelity**: keep unknown fields, wire types and entry order. Keep packed
   fields (6, 36) packed. Write field 11.3 as fixed32 float, not a varint. Keep 64-bit
   two's-complement varints (virtual item payloads) bit-exact. Never write negative values
   as anything but 10-byte two's-complement varints.
8. **Item key byte order** is BE even on PC, and the key is signed. `seed` uses an
   *arithmetic* right shift, and the keystream modulus is 4294967291.
9. **Item checksum**: recompute it after any change to the item bits *or* the key. Pad to 33
   bytes with 0xFF for the CRC (and only for the CRC). Store it BE.
10. **Bit widths**: mask every value to its width before packing. The reference does not mask,
    so an out-of-range level (more than 127) or lib/asset value would corrupt the next field.
11. **Virtual items** (set 255, all else 0, in field 53) are not real items. Don't export
    them, don't relevel them (the reference does with `--forceitemlevels`), and don't drop
    them. They hold the OP-level, playthrough and currency metadata.
12. **XP must be consistent with level**: `T[L] ≤ XP < T[L+1]`, and exactly `T[80]` at
    level 80. The reference's author reports in-game errors when XP overshoots at level
    72. Use the exact table, not the formula.
13. **Backpack/bank**: the absolute sizes (field 13.1, field 56) must match the SDU counts in
    field 36 [7]/[8] (`12 + 3·sdu`, `6 + 2·sdu`). Field 36 must have all 9 entries.
14. **Challenge blob**: endian follows the platform. Keep `size_in_bytes = 12·count + 2` and
    `count` consistent. Preserve the unknown header u32 and the two per-record "1" bytes.
15. **Mission copy**: copying field-18 entries byte for byte copies the playthrough number
    too (5.14).
16. **Strings**: the reference writes Latin-1. Non-ASCII names are untested (**UNSURE**).
17. **Equipped state**: imported weapons default to quick slot 0 and items to unequipped.
    Setting quick slots or equip flags yourself risks clashes (e.g. two weapons in the
    same slot). **UNSURE**: the reference never does this, and how the game resolves
    clashes is unknown.
18. **Field ordering**: the reference re-sorts top-level and modified submessage fields
    by number, and the game accepts that. Writing fields out of numeric order is
    untested.

---

## UNSURE items (consolidated)

1. Mac saves are assumed identical to PC (LE); the reference has no Mac-specific handling.
2. Console byte order for the outer length and WSG `size` field (the reference writes BE; its own comments doubt this for consoles).
3. Purpose of the 4 trailing zero bytes in the WSG blob.
4. Which LZO1X compressor the game uses, so decoders must support the full LZO1X instruction set.
5. Whether the game checks the declared decompressed length, or has a maximum compressed or file size.
6. Typical save size (tens of KB) is a general estimate.
7. Whether the game's Huffman decoder limits code length.
8. Meaning of currency index 3.
9. Units/types of fields 25 (time_played) and 26 (save_timestamp).
10. Field 34 is probably a GUID.
11. Item/weapon "mark" values (0 trash / 1 standard / 2 favourite) come from community knowledge, not the reference.
12. The game's string encoding (the reference writes Latin-1 and reads field 46 as UTF-8).
13. Meaning of field 13.3 (weapon_slots_shown).
14. Virtual item ids 1, 2, 3, 5 (only id 4 = OP level is used in code).
15. Whether the game uses the field-18 position or subfield 1 for the playthrough (relevant to copying NVHM missions).
16. Meaning of the two bytes of the challenge id, read here as one u16.
17. Whether the game verifies the item checksum (almost certainly yes).
18. Item part-slot names, and whether all-ones means "empty part".
19. Whether the trailing pad bits of an item's last byte matter (the reference writes 1s; game samples have 0s).
20. How the game handles inventories or banks over capacity after import.
21. Currency caps (assumed ≤ 2^31−1, non-negative).
22. Equipped-state clashes.
23. Structure of the asset-library manifest needed for item names.

---

## Addendum: findings from a Windows-written save (2026-11-09)

Checked on `Save0001.sav` written by the Windows Steam build (a new level-1 Mechromancer), decoded
both by the Rust `savekit` crate and by an independent throwaway decoder. Where these differ
from the text above, the game-written file wins.

1. **PC and Mac saves are the same format** (resolves UNSURE 1). Every layer decoded with the
   rules above: SHA-1 header, BE length, LZO1X, WSG with LE `version/crc/length`, Huffman,
   protobuf, item blobs.
2. **The four bytes after the Huffman stream are not zero** in this file (§1.2, gotcha 4). The
   WSG blob ended `… d4 93 9f 1a`, with 38 bits left over after the last symbol (6 pad bits plus
   4 bytes). A decoder must ignore them. Writing four zero bytes is still accepted by the game
   (proven in game on the Mac build).
3. **Field 6 (currency) has 13 packed entries**, not 5 (§4.2). The first five are as documented.
   Setters must keep the entries they do not own.
4. **Field 36 (black market) has 7 entries** in a fresh save, so "must have all 9" (§4.2 notes,
   gotcha 13) describes the reference editor, not the game. Pad minimally: to 8 for a backpack
   edit, to 9 for a bank edit, not at all for ammo.
5. **Top-level field 58 exists** (a varint, value 2 here) and is not in the §4.2 table. Unknown
   fields must be preserved, as already required.
6. **Item pad bits are 1 in game-written items** (§6.6, UNSURE 19). Every real item in this file
   has the unused high bits of its last byte set, the opposite of the two README samples.
   Re-encoding keeps whatever the original had.
7. **A blob can carry a whole spare byte** after its last complete field (§6.6): the length is not
   always `ceil(bits / 8)`. Keep the original's extra bytes when the same fields are present.
8. **A new character's backpack can be over capacity.** This save holds 12 unequipped
   customization items (set 9, 9 item bytes each, no manufacturer) plus one unequipped weapon:
   13 entries against 12 slots. The game wrote it that way (relevant to UNSURE 20). Duplicate
   Item counts these entries, so it refuses on a fresh character until the backpack is enlarged.

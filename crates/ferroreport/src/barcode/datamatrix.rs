//! Data Matrix (ECC 200), encoded like barcode4j's `DataMatrixBean`: the same choice of
//! encodation (ASCII, C40, Text) and symbol size, Reed-Solomon error correction and module
//! placement.

#![allow(clippy::type_complexity)]

/// Symbol sizes in the order barcode4j tries them: (rows, columns, data codewords, error
/// codewords, data region rows, data region columns, interleaved blocks, rectangular).
const SIZES: &[(usize, usize, usize, usize, usize, usize, usize, bool)] = &[
    (10, 10, 3, 5, 8, 8, 1, false),
    (12, 12, 5, 7, 10, 10, 1, false),
    (8, 18, 5, 7, 6, 16, 1, true),
    (14, 14, 8, 10, 12, 12, 1, false),
    (8, 32, 10, 11, 6, 14, 1, true),
    (16, 16, 12, 12, 14, 14, 1, false),
    (12, 26, 16, 14, 10, 24, 1, true),
    (18, 18, 18, 14, 16, 16, 1, false),
    (20, 20, 22, 18, 18, 18, 1, false),
    (12, 36, 22, 18, 10, 16, 1, true),
    (22, 22, 30, 20, 20, 20, 1, false),
    (16, 36, 32, 24, 14, 16, 1, true),
    (24, 24, 36, 24, 22, 22, 1, false),
    (26, 26, 44, 28, 24, 24, 1, false),
    (16, 48, 49, 28, 14, 22, 1, true),
    (32, 32, 62, 36, 14, 14, 1, false),
    (36, 36, 86, 42, 16, 16, 1, false),
    (40, 40, 114, 48, 18, 18, 1, false),
    (44, 44, 144, 56, 20, 20, 1, false),
    (48, 48, 174, 68, 22, 22, 1, false),
    (52, 52, 204, 84, 24, 24, 2, false),
    (64, 64, 280, 112, 14, 14, 2, false),
    (72, 72, 368, 144, 16, 16, 4, false),
    (80, 80, 456, 192, 18, 18, 4, false),
    (88, 88, 576, 224, 20, 20, 4, false),
    (96, 96, 696, 272, 22, 22, 4, false),
    (104, 104, 816, 336, 24, 24, 6, false),
    (120, 120, 1050, 408, 18, 18, 6, false),
    (132, 132, 1304, 496, 20, 20, 8, false),
    (144, 144, 1558, 620, 22, 22, 10, false),
];

/// Which symbol shapes may be used (`shape` attribute).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Shape {
    #[default]
    Any,
    Square,
    Rectangle,
}

impl Shape {
    pub fn from_name(v: &str) -> Shape {
        match v {
            "force-square" => Shape::Square,
            "force-rectangle" => Shape::Rectangle,
            _ => Shape::Any,
        }
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Mode {
    Ascii,
    C40,
    Text,
    X12,
    Edifact,
    Base256,
}

const PAD: u8 = 129;
const UNLATCH: u8 = 254;

fn size_for(count: usize, shape: Shape) -> Option<usize> {
    SIZES.iter().position(|s| {
        s.2 >= count
            && match shape {
                Shape::Any => true,
                Shape::Square => !s.7,
                Shape::Rectangle => s.7,
            }
    })
}

fn is_native_c40(c: u8) -> bool {
    c == b' ' || c.is_ascii_digit() || c.is_ascii_uppercase()
}
fn is_native_text(c: u8) -> bool {
    c == b' ' || c.is_ascii_digit() || c.is_ascii_lowercase()
}
fn is_native_x12(c: u8) -> bool {
    matches!(c, b'\r' | b'*' | b'>' | b' ') || c.is_ascii_digit() || c.is_ascii_uppercase()
}
fn is_native_edifact(c: u8) -> bool {
    (b' '..=b'^').contains(&c)
}

/// barcode4j's look-ahead: the encodation that would use the fewest codewords from `start`.
fn look_ahead(msg: &[u8], start: usize, current: Mode) -> Mode {
    if start >= msg.len() {
        return current;
    }
    const ORDER: [Mode; 6] = [Mode::Ascii, Mode::C40, Mode::Text, Mode::X12, Mode::Edifact, Mode::Base256];
    let idx = |m: Mode| ORDER.iter().position(|o| *o == m).unwrap_or(0);
    let mut counts: [f32; 6] = if current == Mode::Ascii {
        [0.0, 1.0, 1.0, 1.0, 1.0, 1.25]
    } else {
        let mut c = [1.0, 2.0, 2.0, 2.0, 2.0, 2.25];
        c[idx(current)] = 0.0;
        c
    };
    let minimums = |counts: &[f32; 6]| -> ([i32; 6], [u8; 6], i32) {
        let ints = counts.map(|c| c.ceil() as i32);
        let min = *ints.iter().min().unwrap_or(&0);
        let mins = ints.map(|i| (i == min) as u8);
        (ints, mins, min)
    };
    let mut processed = 0;
    loop {
        if start + processed == msg.len() {
            let (ints, mins, min) = minimums(&counts);
            let n: u8 = mins.iter().sum();
            if ints[0] == min {
                return Mode::Ascii;
            }
            if n == 1 && mins[5] > 0 {
                return Mode::Base256;
            }
            if n == 1 && mins[4] > 0 {
                return Mode::Edifact;
            }
            if n == 1 && mins[2] > 0 {
                return Mode::Text;
            }
            if n == 1 && mins[3] > 0 {
                return Mode::X12;
            }
            return Mode::C40;
        }
        let c = msg[start + processed];
        processed += 1;
        let ext = c >= 128;
        if c.is_ascii_digit() {
            counts[0] += 0.5;
        } else if ext {
            counts[0] = counts[0].ceil() + 2.0;
        } else {
            counts[0] = counts[0].ceil() + 1.0;
        }
        counts[1] += if is_native_c40(c) {
            2.0 / 3.0
        } else if ext {
            8.0 / 3.0
        } else {
            4.0 / 3.0
        };
        counts[2] += if is_native_text(c) {
            2.0 / 3.0
        } else if ext {
            8.0 / 3.0
        } else {
            4.0 / 3.0
        };
        counts[3] += if is_native_x12(c) {
            2.0 / 3.0
        } else if ext {
            13.0 / 3.0
        } else {
            10.0 / 3.0
        };
        counts[4] += if is_native_edifact(c) {
            3.0 / 4.0
        } else if ext {
            17.0 / 4.0
        } else {
            13.0 / 4.0
        };
        counts[5] += 1.0;
        if processed >= 4 {
            let (ints, mins, _) = minimums(&counts);
            let n: u8 = mins.iter().sum();
            if (1..6).all(|i| ints[0] < ints[i]) {
                return Mode::Ascii;
            }
            if ints[5] < ints[0] || mins[1] + mins[2] + mins[3] + mins[4] == 0 {
                return Mode::Base256;
            }
            if n == 1 && mins[4] > 0 {
                return Mode::Edifact;
            }
            if n == 1 && mins[2] > 0 {
                return Mode::Text;
            }
            if n == 1 && mins[3] > 0 {
                return Mode::X12;
            }
            let c40 = ints[1] + 1;
            if c40 < ints[0] && c40 < ints[5] && c40 < ints[4] && c40 < ints[2] && ints[1] <= ints[3] {
                return Mode::C40;
            }
        }
    }
}

/// The C40 (or Text) values of one character.
fn c40_values(c: u8, text: bool, out: &mut Vec<u8>) -> usize {
    let start = out.len();
    if c >= 128 {
        out.extend([1, 30]);
        c40_values(c - 128, text, out);
        return out.len() - start;
    }
    match c {
        b' ' => out.push(3),
        b'0'..=b'9' => out.push(c - b'0' + 4),
        b'A'..=b'Z' if !text => out.push(c - b'A' + 14),
        b'a'..=b'z' if text => out.push(c - b'a' + 14),
        0..=31 => out.extend([0, c]),
        b'!'..=b'/' => out.extend([1, c - 33]),
        b':'..=b'@' => out.extend([1, c - 58 + 15]),
        b'['..=b'_' => out.extend([1, c - 91 + 22]),
        b'`' if text => out.extend([2, 0]),
        b'A'..=b'Z' => out.extend([2, c - b'A' + 1]),
        b'{'..=127 if text => out.extend([2, c - 123 + 27]),
        _ => out.extend([2, c - 96]),
    }
    out.len() - start
}

/// The data codewords of `msg` and the index of the symbol size that holds them.
pub fn encode(msg: &[u8], shape: Shape) -> Result<(Vec<u8>, usize), String> {
    let mut cw: Vec<u8> = Vec::new();
    let mut pos = 0;
    let mut mode = Mode::Ascii;
    let capacity = |n: usize| size_for(n, shape).map(|i| SIZES[i].2);
    while pos < msg.len() {
        match mode {
            Mode::Ascii => {
                let c = msg[pos];
                if c.is_ascii_digit() && pos + 1 < msg.len() && msg[pos + 1].is_ascii_digit() {
                    cw.push((c - b'0') * 10 + (msg[pos + 1] - b'0') + 130);
                    pos += 2;
                    continue;
                }
                match look_ahead(msg, pos, Mode::Ascii) {
                    Mode::C40 => {
                        cw.push(230);
                        mode = Mode::C40;
                    }
                    Mode::Text => {
                        cw.push(239);
                        mode = Mode::Text;
                    }
                    Mode::Edifact => {
                        cw.push(240);
                        mode = Mode::Edifact;
                    }
                    Mode::X12 => {
                        cw.push(238);
                        mode = Mode::X12;
                    }
                    // Base 256 is written in ASCII here.
                    _ => {
                        if c >= 128 {
                            cw.extend([235, c - 128 + 1]);
                        } else {
                            cw.push(c + 1);
                        }
                        pos += 1;
                    }
                }
            }
            Mode::X12 => {
                let mut buf: Vec<u8> = Vec::new();
                while pos < msg.len() {
                    let c = msg[pos];
                    pos += 1;
                    buf.push(match c {
                        b'\r' => 0,
                        b'*' => 1,
                        b'>' => 2,
                        b' ' => 3,
                        b'0'..=b'9' => c - b'0' + 4,
                        b'A'..=b'Z' => c - b'A' + 14,
                        _ => return Err(format!("Data Matrix X12 cannot encode {:?}", c as char)),
                    });
                    if buf.len() % 3 == 0 {
                        let v = 1600 * buf[0] as u32 + 40 * buf[1] as u32 + buf[2] as u32 + 1;
                        cw.extend([(v / 256) as u8, (v % 256) as u8]);
                        buf.clear();
                        if look_ahead(msg, pos, Mode::X12) != Mode::X12 {
                            break;
                        }
                    }
                }
                let available = capacity(cw.len()).map(|c| c as isize - cw.len() as isize).unwrap_or(0);
                pos -= buf.len();
                let remaining = (msg.len() - pos) as isize;
                if remaining > 1 || available > 1 || remaining != available {
                    cw.push(UNLATCH);
                }
                mode = Mode::Ascii;
            }
            Mode::Edifact => {
                // Four 6-bit values in three codewords.
                let pack = |vals: &[u8]| -> Vec<u8> {
                    let g = |i: usize| vals.get(i).copied().unwrap_or(0) as u32;
                    let v = (g(0) << 18) | (g(1) << 12) | (g(2) << 6) | g(3);
                    let bytes = [(v >> 16) as u8, (v >> 8) as u8, v as u8];
                    bytes[..vals.len().min(3)].to_vec()
                };
                let mut buf: Vec<u8> = Vec::new();
                while pos < msg.len() {
                    let c = msg[pos];
                    if !is_native_edifact(c) {
                        return Err(format!("Data Matrix EDIFACT cannot encode {:?}", c as char));
                    }
                    buf.push(if c >= b'@' { c - 64 } else { c });
                    pos += 1;
                    if buf.len() >= 4 {
                        cw.extend(pack(&buf[..4]));
                        buf.drain(..4);
                        if look_ahead(msg, pos, Mode::Edifact) != Mode::Edifact {
                            break;
                        }
                    }
                }
                buf.push(31);
                let mut done = false;
                if buf.len() == 1 {
                    // Only the unlatch is left: it is left out when the symbol ends anyway.
                    let remaining = msg.len() - pos;
                    let mut available = capacity(cw.len()).map(|c| c as isize - cw.len() as isize).unwrap_or(0);
                    if remaining as isize > available {
                        available = capacity(cw.len() + 1).map(|c| c as isize - cw.len() as isize).unwrap_or(0);
                    }
                    if remaining as isize <= available && available <= 2 {
                        done = true;
                    }
                }
                if !done {
                    let rest = buf.len() - 1;
                    let encoded = pack(&buf);
                    let end = pos >= msg.len();
                    let mut ascii_rest = end && rest <= 2;
                    if rest <= 2 {
                        let available = capacity(cw.len() + rest).map(|c| c as isize - cw.len() as isize).unwrap_or(0);
                        if available >= 3 {
                            ascii_rest = false;
                        }
                    }
                    if ascii_rest {
                        pos -= rest;
                    } else {
                        cw.extend(encoded);
                    }
                }
                mode = Mode::Ascii;
            }
            _ => {
                let text = mode == Mode::Text;
                let mut buf: Vec<u8> = Vec::new();
                let mut sizes: Vec<usize> = Vec::new();
                let mut switched = false;
                while pos < msg.len() {
                    let size = c40_values(msg[pos], text, &mut buf);
                    sizes.push(size);
                    pos += 1;
                    let unwritten = buf.len() / 3 * 2;
                    let count = cw.len() + unwritten;
                    let available = capacity(count).map(|c| c as isize - count as isize).unwrap_or(0);
                    if pos >= msg.len() {
                        // Avoid a single value in the last triplet.
                        let backtrack = |buf: &mut Vec<u8>, sizes: &mut Vec<usize>, pos: &mut usize| {
                            let n = sizes.pop().unwrap_or(0);
                            buf.truncate(buf.len() - n);
                            *pos -= 1;
                        };
                        if buf.len() % 3 == 2 && available != 2 {
                            backtrack(&mut buf, &mut sizes, &mut pos);
                        }
                        while buf.len() % 3 == 1 && sizes.last().is_some_and(|&s| s > 3 || available != 1) {
                            backtrack(&mut buf, &mut sizes, &mut pos);
                        }
                        break;
                    }
                    if buf.len() % 3 == 0 && look_ahead(msg, pos, mode) != mode {
                        switched = true;
                        break;
                    }
                }
                // Write the triplets.
                let rest = buf.len() % 3;
                let unwritten = buf.len() / 3 * 2;
                let count = cw.len() + unwritten;
                let available = capacity(count).map(|c| c as isize - count as isize).unwrap_or(0);
                let more = pos < msg.len();
                if rest == 2 {
                    buf.push(0);
                }
                let mut i = 0;
                while i + 3 <= buf.len() {
                    let v = 1600 * buf[i] as u32 + 40 * buf[i + 1] as u32 + buf[i + 2] as u32 + 1;
                    cw.extend([(v / 256) as u8, (v % 256) as u8]);
                    i += 3;
                }
                match rest {
                    // One value left over: its character goes in ASCII.
                    1 => {
                        pos -= 1;
                        if more || available != 1 {
                            cw.push(UNLATCH);
                        }
                    }
                    2 => {
                        if more {
                            cw.push(UNLATCH);
                        }
                    }
                    _ => {
                        if more || switched || available > 0 {
                            cw.push(UNLATCH);
                        }
                    }
                }
                mode = Mode::Ascii;
            }
        }
    }
    let size = size_for(cw.len(), shape).ok_or_else(|| "the message is too long for a Data Matrix symbol".to_string())?;
    let capacity = SIZES[size].2;
    if cw.len() < capacity {
        cw.push(PAD);
    }
    while cw.len() < capacity {
        let position = cw.len() + 1;
        let r = (149 * position) % 253 + 1;
        let v = PAD as usize + r;
        cw.push(if v <= 254 { v } else { v - 254 } as u8);
    }
    Ok((cw, size))
}

/// Reed-Solomon error codewords over GF(256) with the polynomial 0x12D.
fn ecc(data: &[u8], n: usize) -> Vec<u8> {
    let mut exp = [0u8; 512];
    let mut log = [0u8; 256];
    let mut x: u16 = 1;
    for (i, e) in exp.iter_mut().enumerate().take(255) {
        *e = x as u8;
        log[x as usize] = i as u8;
        x <<= 1;
        if x & 0x100 != 0 {
            x ^= 0x12D;
        }
    }
    for i in 255..512 {
        exp[i] = exp[i - 255];
    }
    let mul = |a: u8, b: u8| if a == 0 || b == 0 { 0 } else { exp[log[a as usize] as usize + log[b as usize] as usize] };
    // Generator: (x - a^1)(x - a^2)...(x - a^n), highest power first.
    let mut g = vec![1u8];
    for &root in &exp[1..=n] {
        let mut next = vec![0u8; g.len() + 1];
        for (j, &c) in g.iter().enumerate() {
            next[j] ^= c;
            next[j + 1] ^= mul(c, root);
        }
        g = next;
    }
    let mut rem = vec![0u8; n];
    for &d in data {
        let factor = d ^ rem[0];
        rem.remove(0);
        rem.push(0);
        for k in 0..n {
            rem[k] ^= mul(g[k + 1], factor);
        }
    }
    rem
}

/// Data and error codewords interleaved as the symbol stores them.
fn with_ecc(data: &[u8], size: usize) -> Vec<u8> {
    let (_, _, capacity, errors, _, _, blocks, _) = SIZES[size];
    let mut out = data.to_vec();
    out.resize(capacity + errors, 0);
    let per_block = errors / blocks;
    for b in 0..blocks {
        let block: Vec<u8> = (b..capacity).step_by(blocks).map(|i| data[i]).collect();
        for (k, e) in ecc(&block, per_block).into_iter().enumerate() {
            out[capacity + b + k * blocks] = e;
        }
    }
    out
}

/// The ECC 200 placement of codewords in the data area (`numrows`×`numcols`).
fn place(codewords: &[u8], numrows: usize, numcols: usize) -> Vec<Option<bool>> {
    let (nr, nc) = (numrows as isize, numcols as isize);
    let mut bits: Vec<Option<bool>> = vec![None; numrows * numcols];
    let mut module = |bits: &mut Vec<Option<bool>>, mut row: isize, mut col: isize, pos: usize, bit: u32| {
        if row < 0 {
            row += nr;
            col += 4 - ((nr + 4) % 8);
        }
        if col < 0 {
            col += nc;
            row += 4 - ((nc + 4) % 8);
        }
        let v = codewords.get(pos).copied().unwrap_or(0);
        bits[(row * nc + col) as usize] = Some(v & (1 << (8 - bit)) != 0);
    };
    let utah = |bits: &mut Vec<Option<bool>>,
                m: &mut dyn FnMut(&mut Vec<Option<bool>>, isize, isize, usize, u32),
                r: isize,
                c: isize,
                p: usize| {
        m(bits, r - 2, c - 2, p, 1);
        m(bits, r - 2, c - 1, p, 2);
        m(bits, r - 1, c - 2, p, 3);
        m(bits, r - 1, c - 1, p, 4);
        m(bits, r - 1, c, p, 5);
        m(bits, r, c - 2, p, 6);
        m(bits, r, c - 1, p, 7);
        m(bits, r, c, p, 8);
    };
    let corner = |bits: &mut Vec<Option<bool>>,
                  m: &mut dyn FnMut(&mut Vec<Option<bool>>, isize, isize, usize, u32),
                  cells: [(isize, isize); 8],
                  p: usize| {
        for (i, (r, c)) in cells.iter().enumerate() {
            m(bits, *r, *c, p, i as u32 + 1);
        }
    };
    let has = |bits: &Vec<Option<bool>>, row: isize, col: isize| bits[(row * nc + col) as usize].is_some();
    let (mut pos, mut row, mut col) = (0usize, 4isize, 0isize);
    loop {
        if row == nr && col == 0 {
            corner(
                &mut bits,
                &mut module,
                [(nr - 1, 0), (nr - 1, 1), (nr - 1, 2), (0, nc - 2), (0, nc - 1), (1, nc - 1), (2, nc - 1), (3, nc - 1)],
                pos,
            );
            pos += 1;
        }
        if row == nr - 2 && col == 0 && nc % 4 != 0 {
            corner(
                &mut bits,
                &mut module,
                [(nr - 3, 0), (nr - 2, 0), (nr - 1, 0), (0, nc - 4), (0, nc - 3), (0, nc - 2), (0, nc - 1), (1, nc - 1)],
                pos,
            );
            pos += 1;
        }
        if row == nr - 2 && col == 0 && nc % 8 == 4 {
            corner(
                &mut bits,
                &mut module,
                [(nr - 3, 0), (nr - 2, 0), (nr - 1, 0), (0, nc - 2), (0, nc - 1), (1, nc - 1), (2, nc - 1), (3, nc - 1)],
                pos,
            );
            pos += 1;
        }
        if row == nr + 4 && col == 2 && nc % 8 == 0 {
            corner(
                &mut bits,
                &mut module,
                [(nr - 1, 0), (nr - 1, nc - 1), (0, nc - 3), (0, nc - 2), (0, nc - 1), (1, nc - 3), (1, nc - 2), (1, nc - 1)],
                pos,
            );
            pos += 1;
        }
        loop {
            if row < nr && col >= 0 && !has(&bits, row, col) {
                utah(&mut bits, &mut module, row, col, pos);
                pos += 1;
            }
            row -= 2;
            col += 2;
            if !(row >= 0 && col < nc) {
                break;
            }
        }
        row += 1;
        col += 3;
        loop {
            if row >= 0 && col < nc && !has(&bits, row, col) {
                utah(&mut bits, &mut module, row, col, pos);
                pos += 1;
            }
            row += 2;
            col -= 2;
            if !(row < nr && col >= 0) {
                break;
            }
        }
        row += 3;
        col += 1;
        if !(row < nr || col < nc) {
            break;
        }
    }
    if !has(&bits, nr - 1, nc - 1) {
        bits[((nr - 1) * nc + nc - 1) as usize] = Some(true);
        bits[((nr - 2) * nc + nc - 2) as usize] = Some(true);
    }
    bits
}

/// The modules of the symbol for `msg`, row by row (`true`: dark), with its rows and columns.
pub fn matrix(msg: &str, shape: Shape) -> Result<(Vec<bool>, usize, usize), String> {
    let bytes: Vec<u8> = msg
        .chars()
        .map(|c| if (c as u32) < 256 { Ok(c as u8) } else { Err(format!("Data Matrix cannot encode {c:?}")) })
        .collect::<Result<_, _>>()?;
    let (data, size) = encode(&bytes, shape)?;
    let (rows, cols, _, _, rh, rw, _, _) = SIZES[size];
    let codewords = with_ecc(&data, size);
    let (vregions, hregions) = (rows / (rh + 2), cols / (rw + 2));
    let (numrows, numcols) = (vregions * rh, hregions * rw);
    let bits = place(&codewords, numrows, numcols);
    let mut m = vec![false; rows * cols];
    let mut my = 0;
    for y in 0..numrows {
        if y % rh == 0 {
            for x in 0..cols {
                m[my * cols + x] = x % 2 == 0;
            }
            my += 1;
        }
        let mut mx = 0;
        for x in 0..numcols {
            if x % rw == 0 {
                m[my * cols + mx] = true;
                mx += 1;
            }
            m[my * cols + mx] = bits[y * numcols + x] == Some(true);
            mx += 1;
            if x % rw == rw - 1 {
                m[my * cols + mx] = y % 2 == 0;
                mx += 1;
            }
        }
        my += 1;
        if y % rh == rh - 1 {
            for x in 0..cols {
                m[my * cols + x] = true;
            }
            my += 1;
        }
    }
    Ok((m, rows, cols))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reed_solomon_matches_the_standard_example() {
        // ISO/IEC 16022 annex: "123456" in a 10x10 symbol.
        let (data, size) = encode(b"123456", Shape::Any).unwrap();
        assert_eq!(data, vec![142, 164, 186]);
        assert_eq!(size, 0);
        assert_eq!(ecc(&data, 5), vec![114, 25, 5, 88, 102]);
    }
}

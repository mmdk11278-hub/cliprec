use std::env;
use std::fs;
use std::path::{Path, PathBuf};

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=assets/cliprec.ico");
    println!("cargo:rerun-if-changed=target/release/cliprec.exe");

    let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());
    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());

    // 1. Copy release cliprec.exe payload for ClipRec-Setup.exe (or empty placeholder)
    let payload_src = manifest_dir.join("target").join("release").join("cliprec.exe");
    let payload_dst = out_dir.join("cliprec_payload.bin");
    if payload_src.is_file() {
        let _ = fs::copy(&payload_src, &payload_dst);
    } else {
        let _ = fs::write(&payload_dst, b"");
    }

    // 2. Ensure assets/cliprec.ico exists (multi-res uncompressed/BMP-less? No, PNG-in-ICO 256/64/48/32/16)
    let assets_dir = manifest_dir.join("assets");
    let _ = fs::create_dir_all(&assets_dir);
    let ico_path = assets_dir.join("cliprec.ico");
    let ico_bytes = build_ico_bytes();
    let _ = fs::write(&ico_path, &ico_bytes);

    // 3. Build x86_64 COFF .rsrc object for Windows PE icon embedding
    if env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let obj_path = out_dir.join("icon_res.obj");
        let obj_bytes = build_coff_rsrc_from_ico(&ico_bytes);
        fs::write(&obj_path, obj_bytes).expect("write icon_res.obj");
        println!("cargo:rustc-link-arg={}", obj_path.display());
    }
}

/// Renders a crisp 32-bit RGBA brand icon (purple squircle + white ring + red dot).
fn render_icon_rgba(size: u32) -> Vec<u8> {
    let s = size.max(1) as f32;
    let mut rgba = vec![0u8; (size * size * 4) as usize];
    let half = s * 0.5;
    let corner_r = s * 0.22;
    let box_half = half - s * 0.04;
    let ring_r = s * 0.29;
    let ring_thick = (s * 0.075).max(1.2);
    let dot_r = s * 0.135;

    for y in 0..size {
        for x in 0..size {
            let px = x as f32 + 0.5 - half;
            let py = y as f32 + 0.5 - half;

            let qx = px.abs() - (box_half - corner_r);
            let qy = py.abs() - (box_half - corner_r);
            let outside = (qx.max(0.0).powi(2) + qy.max(0.0).powi(2)).sqrt() + qx.max(qy).min(0.0)
                - corner_r;

            let bg_alpha = (0.5 - outside).clamp(0.0, 1.0);
            if bg_alpha <= 0.0 {
                continue;
            }

            let t = ((x as f32 / s) * 0.55 + (y as f32 / s) * 0.45).clamp(0.0, 1.0);
            let mut r = 81.0 + (127.0 - 81.0) * t;
            let mut g = 0.0 + 84.0 * t;
            let mut b = 164.0 + (248.0 - 164.0) * t;

            if outside > -s * 0.035 {
                r = (r + 28.0).min(255.0);
                g = (g + 24.0).min(255.0);
                b = (b + 36.0).min(255.0);
            }

            let dist = (px * px + py * py).sqrt();
            let ring_dist = (dist - ring_r).abs() - ring_thick * 0.5;
            let ring_alpha = (0.5 - ring_dist).clamp(0.0, 1.0);
            if ring_alpha > 0.0 {
                r = r * (1.0 - ring_alpha) + 255.0 * ring_alpha;
                g = g * (1.0 - ring_alpha) + 255.0 * ring_alpha;
                b = b * (1.0 - ring_alpha) + 255.0 * ring_alpha;
            }

            let dot_alpha = (0.5 - (dist - dot_r)).clamp(0.0, 1.0);
            if dot_alpha > 0.0 {
                r = r * (1.0 - dot_alpha) + 255.0 * dot_alpha;
                g = g * (1.0 - dot_alpha) + 59.0 * dot_alpha;
                b = b * (1.0 - dot_alpha) + 48.0 * dot_alpha;
            }

            let idx = ((y * size + x) * 4) as usize;
            rgba[idx] = r.round().clamp(0.0, 255.0) as u8;
            rgba[idx + 1] = g.round().clamp(0.0, 255.0) as u8;
            rgba[idx + 2] = b.round().clamp(0.0, 255.0) as u8;
            rgba[idx + 3] = (bg_alpha * 255.0).round().clamp(0.0, 255.0) as u8;
        }
    }
    rgba
}

/// Encodes a minimal uncompressed 32-bit BGRA BMP-style DIB (no PNG dependency needed in build.rs)
/// for `.ico` / `RT_ICON` frames so every Windows version and Explorer shell renders it natively.
fn encode_ico_dib(size: u32) -> Vec<u8> {
    let rgba = render_icon_rgba(size);
    let mut dib = Vec::new();
    // BITMAPINFOHEADER (40 bytes)
    dib.extend_from_slice(&40u32.to_le_bytes()); // biSize
    dib.extend_from_slice(&(size as i32).to_le_bytes()); // biWidth
    dib.extend_from_slice(&((size * 2) as i32).to_le_bytes()); // biHeight (XOR + AND)
    dib.extend_from_slice(&1u16.to_le_bytes()); // biPlanes
    dib.extend_from_slice(&32u16.to_le_bytes()); // biBitCount
    dib.extend_from_slice(&0u32.to_le_bytes()); // biCompression = BI_RGB
    dib.extend_from_slice(&(size * size * 4).to_le_bytes()); // biSizeImage
    dib.extend_from_slice(&0i32.to_le_bytes()); // biXPelsPerMeter
    dib.extend_from_slice(&0i32.to_le_bytes()); // biYPelsPerMeter
    dib.extend_from_slice(&0u32.to_le_bytes()); // biClrUsed
    dib.extend_from_slice(&0u32.to_le_bytes()); // biClrImportant

    // XOR mask: bottom-up BGRA
    for y in (0..size).rev() {
        for x in 0..size {
            let idx = ((y * size + x) * 4) as usize;
            dib.push(rgba[idx + 2]); // B
            dib.push(rgba[idx + 1]); // G
            dib.push(rgba[idx]); // R
            dib.push(rgba[idx + 3]); // A
        }
    }

    // AND mask: 1bpp bottom-up, rows padded to 4 bytes
    let row_bytes = ((size + 31) / 32) * 4;
    dib.resize(dib.len() + (row_bytes * size) as usize, 0);
    dib
}

fn build_ico_bytes() -> Vec<u8> {
    let sizes = [64u32, 48, 32, 16];
    let dibs: Vec<(u32, Vec<u8>)> = sizes.iter().map(|&s| (s, encode_ico_dib(s))).collect();
    let count = dibs.len() as u16;
    let mut out = Vec::new();
    out.extend_from_slice(&0u16.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&count.to_le_bytes());

    let mut offset = 6u32 + (count as u32) * 16;
    for (sz, data) in &dibs {
        out.push(*sz as u8);
        out.push(*sz as u8);
        out.push(0);
        out.push(0);
        out.extend_from_slice(&1u16.to_le_bytes());
        out.extend_from_slice(&32u16.to_le_bytes());
        out.extend_from_slice(&(data.len() as u32).to_le_bytes());
        out.extend_from_slice(&offset.to_le_bytes());
        offset += data.len() as u32;
    }
    for (_, data) in dibs {
        out.extend_from_slice(&data);
    }
    out
}

/// Builds a valid x86_64 COFF object file containing a `.rsrc` section with
/// `RT_ICON` (type 3) and `RT_GROUP_ICON` (type 14, ID 1) parsed from `ico`.
fn build_coff_rsrc_from_ico(ico: &[u8]) -> Vec<u8> {
    let count = u16::from_le_bytes([ico[4], ico[5]]) as usize;
    let mut icon_frames: Vec<(u8, u8, u16, u16, &[u8])> = Vec::with_capacity(count);
    for i in 0..count {
        let base = 6 + i * 16;
        let w = ico[base];
        let h = ico[base + 1];
        let planes = u16::from_le_bytes([ico[base + 4], ico[base + 5]]);
        let bpp = u16::from_le_bytes([ico[base + 6], ico[base + 7]]);
        let size = u32::from_le_bytes([ico[base + 8], ico[base + 9], ico[base + 10], ico[base + 11]]) as usize;
        let off = u32::from_le_bytes([ico[base + 12], ico[base + 13], ico[base + 14], ico[base + 15]]) as usize;
        icon_frames.push((w, h, planes, bpp, &ico[off..off + size]));
    }

    // Build GRPICONDIR (6 + count * 14 bytes)
    let mut grp = Vec::new();
    grp.extend_from_slice(&0u16.to_le_bytes());
    grp.extend_from_slice(&1u16.to_le_bytes());
    grp.extend_from_slice(&(count as u16).to_le_bytes());
    for (idx, (w, h, planes, bpp, data)) in icon_frames.iter().enumerate() {
        grp.push(*w);
        grp.push(*h);
        grp.push(0);
        grp.push(0);
        grp.extend_from_slice(&planes.to_le_bytes());
        grp.extend_from_slice(&bpp.to_le_bytes());
        grp.extend_from_slice(&(data.len() as u32).to_le_bytes());
        grp.extend_from_slice(&((idx + 1) as u16).to_le_bytes()); // nId = 1..=count
    }

    // Build .rsrc section bytes:
    // Root dir (16 + 2*8 = 32 bytes): [type 3 (RT_ICON), type 14 (RT_GROUP_ICON)]
    // Type 3 dir (16 + count*8 bytes)
    // Type 14 dir (16 + 1*8 = 24 bytes)
    // Lang dirs: (count + 1) * (16 + 8 = 24 bytes)
    // Data entries: (count + 1) * 16 bytes
    // Raw data blobs (4-byte aligned)
    let root_size = 16 + 2 * 8;
    let type3_off = root_size;
    let type3_size = 16 + count * 8;
    let type14_off = type3_off + type3_size;
    let type14_size = 16 + 8;
    let lang_base = type14_off + type14_size;
    let data_entry_base = lang_base + (count + 1) * 24;
    let mut blob_off = data_entry_base + (count + 1) * 16;

    let mut rsrc = vec![0u8; blob_off];
    let write_dir = |buf: &mut [u8], off: usize, n_id: u16| {
        buf[off + 14..off + 16].copy_from_slice(&n_id.to_le_bytes());
    };
    let write_entry = |buf: &mut [u8], off: usize, id: u32, target: u32, is_dir: bool| {
        buf[off..off + 4].copy_from_slice(&id.to_le_bytes());
        let val = if is_dir { target | 0x8000_0000 } else { target };
        buf[off + 4..off + 8].copy_from_slice(&val.to_le_bytes());
    };

    // Root
    write_dir(&mut rsrc, 0, 2);
    write_entry(&mut rsrc, 16, 3, type3_off as u32, true);
    write_entry(&mut rsrc, 24, 14, type14_off as u32, true);

    // Type 3 (RT_ICON)
    write_dir(&mut rsrc, type3_off, count as u16);
    for i in 0..count {
        let lang_off = lang_base + i * 24;
        let de_off = data_entry_base + i * 16;
        write_entry(&mut rsrc, type3_off + 16 + i * 8, (i + 1) as u32, lang_off as u32, true);
        write_dir(&mut rsrc, lang_off, 1);
        write_entry(&mut rsrc, lang_off + 16, 0x0409, de_off as u32, false);
    }

    // Type 14 (RT_GROUP_ICON)
    let grp_lang_off = lang_base + count * 24;
    let grp_de_off = data_entry_base + count * 16;
    write_dir(&mut rsrc, type14_off, 1);
    write_entry(&mut rsrc, type14_off + 16, 1, grp_lang_off as u32, true);
    write_dir(&mut rsrc, grp_lang_off, 1);
    write_entry(&mut rsrc, grp_lang_off + 16, 0x0409, grp_de_off as u32, false);

    // Data entries + blobs + COFF relocations
    let mut relocs: Vec<(u32, u32, u16)> = Vec::new();
    for (i, (_, _, _, _, data)) in icon_frames.iter().enumerate() {
        let de_off = data_entry_base + i * 16;
        rsrc[de_off..de_off + 4].copy_from_slice(&(blob_off as u32).to_le_bytes());
        rsrc[de_off + 4..de_off + 8].copy_from_slice(&(data.len() as u32).to_le_bytes());
        relocs.push((de_off as u32, 0, 0x0003)); // IMAGE_REL_AMD64_ADDR32NB
        rsrc.extend_from_slice(data);
        while rsrc.len() % 4 != 0 {
            rsrc.push(0);
        }
        blob_off = rsrc.len();
    }
    rsrc[grp_de_off..grp_de_off + 4].copy_from_slice(&(blob_off as u32).to_le_bytes());
    rsrc[grp_de_off + 4..grp_de_off + 8].copy_from_slice(&(grp.len() as u32).to_le_bytes());
    relocs.push((grp_de_off as u32, 0, 0x0003));
    rsrc.extend_from_slice(&grp);
    while rsrc.len() % 4 != 0 {
        rsrc.push(0);
    }

    // Assemble x86_64 COFF object: 20B header + 40B section header + rsrc + relocs + symtab(18B) + strtab(4B)
    let raw_ptr = 20u32 + 40u32;
    let reloc_ptr = raw_ptr + rsrc.len() as u32;
    let sym_ptr = reloc_ptr + (relocs.len() as u32) * 10;

    let mut obj = Vec::new();
    // IMAGE_FILE_HEADER
    obj.extend_from_slice(&0x8664u16.to_le_bytes()); // Machine: AMD64
    obj.extend_from_slice(&1u16.to_le_bytes()); // NumberOfSections
    obj.extend_from_slice(&0u32.to_le_bytes()); // TimeDateStamp
    obj.extend_from_slice(&sym_ptr.to_le_bytes()); // PointerToSymbolTable
    obj.extend_from_slice(&1u32.to_le_bytes()); // NumberOfSymbols
    obj.extend_from_slice(&0u16.to_le_bytes()); // SizeOfOptionalHeader
    obj.extend_from_slice(&0u16.to_le_bytes()); // Characteristics

    // IMAGE_SECTION_HEADER (.rsrc)
    obj.extend_from_slice(b".rsrc\0\0\0");
    obj.extend_from_slice(&0u32.to_le_bytes()); // VirtualSize
    obj.extend_from_slice(&0u32.to_le_bytes()); // VirtualAddress
    obj.extend_from_slice(&(rsrc.len() as u32).to_le_bytes()); // SizeOfRawData
    obj.extend_from_slice(&raw_ptr.to_le_bytes()); // PointerToRawData
    obj.extend_from_slice(&reloc_ptr.to_le_bytes()); // PointerToRelocations
    obj.extend_from_slice(&0u32.to_le_bytes()); // PointerToLinenumbers
    obj.extend_from_slice(&(relocs.len() as u16).to_le_bytes()); // NumberOfRelocations
    obj.extend_from_slice(&0u16.to_le_bytes()); // NumberOfLinenumbers
    obj.extend_from_slice(&0x4000_0040u32.to_le_bytes()); // CNT_INITIALIZED_DATA | MEM_READ

    obj.extend_from_slice(&rsrc);
    for (va, sym, rtype) in relocs {
        obj.extend_from_slice(&va.to_le_bytes());
        obj.extend_from_slice(&sym.to_le_bytes());
        obj.extend_from_slice(&rtype.to_le_bytes());
    }
    // Symbol 0: ".rsrc" (section 1, static)
    obj.extend_from_slice(b".rsrc\0\0\0");
    obj.extend_from_slice(&0u32.to_le_bytes());
    obj.extend_from_slice(&1i16.to_le_bytes());
    obj.extend_from_slice(&0u16.to_le_bytes());
    obj.push(3); // IMAGE_SYM_CLASS_STATIC
    obj.push(0); // NumberOfAuxSymbols
    // String table (4 bytes = empty)
    obj.extend_from_slice(&4u32.to_le_bytes());
    obj
}

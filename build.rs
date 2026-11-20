//! Сборка: вшивает в Zakladka.exe значок (та же лента-закладка, что в трее) и сведения о файле
//! (название «Закладка», версия из Cargo.toml) — их показывают меню «Пуск», Проводник, ярлыки и
//! диспетчер задач. Внешние программы (windres, rc.exe) не нужны: ресурсы собираются здесь в
//! объектный файл COFF с разделом .rsrc, и компоновщик получает его только для exe.

use std::env;
use std::fs;
use std::path::PathBuf;

#[path = "src/core/ribbon.rs"]
mod ribbon;

/// Цвет значка — акцент по умолчанию (`bar_color` #FF7A00), как у значка трея без настроек.
const ACCENT: [u8; 3] = [0xFF, 0x7A, 0x00];
/// Размеры картинок значка. До 64 — растр BMP, 256 — PNG (так их хранит сама Windows).
const SIZES: [u32; 8] = [16, 20, 24, 32, 40, 48, 64, 256];
/// Проб на сторону пикселя: край выреза сглажен по доле площади пикселя внутри ленты.
const SAMPLES: u32 = 8;

const RT_ICON: u32 = 3;
const RT_GROUP_ICON: u32 = 14;
const RT_VERSION: u32 = 16;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=src/core/ribbon.rs");
    let target = |key: &str| env::var(key).unwrap_or_default();
    // Объект собран для компоновщика GNU (тулчейн проекта — x86_64-pc-windows-gnu).
    if target("CARGO_CFG_TARGET_OS") != "windows"
        || target("CARGO_CFG_TARGET_ARCH") != "x86_64"
        || target("CARGO_CFG_TARGET_ENV") != "gnu"
    {
        println!("cargo:warning=значок и сведения о файле вшиваются только для x86_64-pc-windows-gnu");
        return;
    }

    let mut resources: Vec<(u32, u32, Vec<u8>)> = Vec::new();
    let mut group = Vec::new();
    group.extend(0u16.to_le_bytes()); // idReserved
    group.extend(1u16.to_le_bytes()); // idType: значок
    group.extend((SIZES.len() as u16).to_le_bytes());
    for (index, &size) in SIZES.iter().enumerate() {
        let id = index as u32 + 1;
        let pixels = rgba(size);
        let image = if size >= 256 { png(size, &pixels) } else { dib(size, &pixels) };
        let side = if size >= 256 { 0 } else { size as u8 }; // 0 означает 256
        group.extend([side, side, 0, 0]); // ширина, высота, цветов в палитре, резерв
        group.extend(1u16.to_le_bytes()); // плоскостей
        group.extend(32u16.to_le_bytes()); // бит на пиксель
        group.extend((image.len() as u32).to_le_bytes());
        group.extend((id as u16).to_le_bytes());
        resources.push((RT_ICON, id, image));
    }
    resources.push((RT_GROUP_ICON, 1, group));
    resources.push((RT_VERSION, 1, version_info(&target("CARGO_PKG_VERSION"))));

    let (section, relocations) = resource_section(&mut resources);
    let out = PathBuf::from(target("OUT_DIR")).join("zakladka-resources.o");
    fs::write(&out, coff(&section, &relocations)).expect("запись объекта ресурсов");
    println!("cargo:rustc-link-arg-bins={}", out.display());
}

/// RGBA сверху вниз; альфа — доля площади пикселя внутри ленты.
fn rgba(size: u32) -> Vec<u8> {
    let mut out = Vec::with_capacity((size * size * 4) as usize);
    for y in 0..size {
        for x in 0..size {
            let mut hits = 0;
            for sy in 0..SAMPLES {
                for sx in 0..SAMPLES {
                    let u = (x as f64 + (sx as f64 + 0.5) / SAMPLES as f64) / size as f64;
                    let v = (y as f64 + (sy as f64 + 0.5) / SAMPLES as f64) / size as f64;
                    if ribbon::inside(u, v) {
                        hits += 1;
                    }
                }
            }
            let total = SAMPLES * SAMPLES;
            let alpha = (hits * 255 + total / 2) / total;
            out.extend_from_slice(&[ACCENT[0], ACCENT[1], ACCENT[2], alpha as u8]);
        }
    }
    out
}

/// Картинка значка в формате DIB: BITMAPINFOHEADER (высота удвоена), цвет BGRA снизу вверх,
/// затем маска 1 бит на пиксель (1 — прозрачно) со строками по 32 бита.
fn dib(size: u32, rgba: &[u8]) -> Vec<u8> {
    let n = size as usize;
    let mask_row = n.div_ceil(32) * 4;
    let mut d = Vec::new();
    d.extend(40u32.to_le_bytes());
    d.extend((size as i32).to_le_bytes());
    d.extend((2 * size as i32).to_le_bytes());
    d.extend(1u16.to_le_bytes());
    d.extend(32u16.to_le_bytes());
    d.extend(0u32.to_le_bytes()); // BI_RGB
    d.extend(((n * n * 4 + mask_row * n) as u32).to_le_bytes());
    d.extend([0u8; 16]); // разрешение и палитра не заданы
    for y in (0..n).rev() {
        for x in 0..n {
            let p = &rgba[(y * n + x) * 4..][..4];
            d.extend([p[2], p[1], p[0], p[3]]);
        }
    }
    for y in (0..n).rev() {
        let mut row = vec![0u8; mask_row];
        for x in 0..n {
            if rgba[(y * n + x) * 4 + 3] == 0 {
                row[x / 8] |= 0x80 >> (x % 8);
            }
        }
        d.extend(row);
    }
    d
}

fn png(size: u32, rgba: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut out, size, size);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        encoder.set_compression(png::Compression::Best);
        let mut writer = encoder.write_header().expect("PNG: заголовок");
        writer.write_image_data(rgba).expect("PNG: данные");
        writer.finish().expect("PNG: конец");
    }
    out
}

fn utf16z(s: &str) -> Vec<u8> {
    s.encode_utf16().chain([0]).flat_map(u16::to_le_bytes).collect()
}

fn pad4(v: &mut Vec<u8>) {
    while v.len() % 4 != 0 {
        v.push(0);
    }
}

/// Узел VERSIONINFO: wLength, wValueLength, wType (1 — текст), ключ, значение, дети; всё по 4 байта.
fn node(key: &str, value: &[u8], value_len: u16, text: bool, children: &[Vec<u8>]) -> Vec<u8> {
    let mut v = vec![0u8; 6];
    v.extend(utf16z(key));
    pad4(&mut v);
    v.extend_from_slice(value);
    for child in children {
        pad4(&mut v);
        v.extend_from_slice(child);
    }
    let length = v.len() as u16;
    v[0..2].copy_from_slice(&length.to_le_bytes());
    v[2..4].copy_from_slice(&value_len.to_le_bytes());
    v[4..6].copy_from_slice(&(text as u16).to_le_bytes());
    v
}

/// Сведения о файле (вкладка «Подробно» в свойствах, имя процесса в диспетчере задач).
fn version_info(version: &str) -> Vec<u8> {
    let mut parts = version.split(['.', '-', '+']).map(|p| p.parse::<u32>().unwrap_or(0));
    let (major, minor, patch) = (
        parts.next().unwrap_or(0),
        parts.next().unwrap_or(0),
        parts.next().unwrap_or(0),
    );
    let (ms, ls) = ((major << 16) | minor, patch << 16);
    // VS_FIXEDFILEINFO: подпись, версия структуры, версии файла и продукта, флаги, ОС Windows NT,
    // тип — программа.
    let fixed: [u32; 13] = [0xFEEF_04BD, 0x0001_0000, ms, ls, ms, ls, 0x3F, 0, 0x0004_0004, 1, 0, 0, 0];
    let fixed: Vec<u8> = fixed.iter().flat_map(|x| x.to_le_bytes()).collect();
    let strings = [
        ("CompanyName", "Zakladka contributors"),
        ("FileDescription", "Закладка"),
        ("FileVersion", version),
        ("InternalName", "Zakladka"),
        ("LegalCopyright", "MIT License, Zakladka contributors"),
        ("OriginalFilename", "Zakladka.exe"),
        ("ProductName", "Закладка"),
        ("ProductVersion", version),
        ("Comments", "Отметка в выделенном тексте любого окна Windows"),
    ];
    let strings: Vec<Vec<u8>> = strings
        .iter()
        .map(|(key, text)| {
            let value = utf16z(text);
            node(key, &value, (value.len() / 2) as u16, true, &[])
        })
        .collect();
    // Русский (0x0419), Юникод (1200 = 0x04B0).
    let table = node("041904B0", &[], 0, true, &strings);
    let string_info = node("StringFileInfo", &[], 0, true, &[table]);
    let translation: Vec<u8> = [0x0419u16, 0x04B0].iter().flat_map(|x| x.to_le_bytes()).collect();
    let var = node("Translation", &translation, 4, false, &[]);
    let var_info = node("VarFileInfo", &[], 0, true, &[var]);
    node("VS_VERSION_INFO", &fixed, fixed.len() as u16, false, &[string_info, var_info])
}

/// Раздел .rsrc: дерево каталогов тип → номер → язык (нейтральный), записи данных, сами данные.
/// Возвращает раздел и смещения полей OffsetToData, которым нужна поправка компоновщика.
fn resource_section(resources: &mut [(u32, u32, Vec<u8>)]) -> (Vec<u8>, Vec<u32>) {
    resources.sort_by_key(|r| (r.0, r.1));
    let mut types: Vec<u32> = resources.iter().map(|r| r.0).collect();
    types.dedup();
    let directory = |entries: usize| 16 + 8 * entries;
    let level1 = directory(types.len());
    let level2: Vec<usize> = types
        .iter()
        .map(|&t| directory(resources.iter().filter(|r| r.0 == t).count()))
        .collect();
    let level2_start = level1;
    let level3_start = level2_start + level2.iter().sum::<usize>();
    let entries_start = level3_start + resources.len() * directory(1);
    let mut data_start = entries_start + resources.len() * 16;
    data_start = data_start.div_ceil(8) * 8;

    let header = |v: &mut Vec<u8>, ids: usize| {
        v.extend([0u8; 12]); // характеристики, время, версия
        v.extend(0u16.to_le_bytes()); // записей с именами
        v.extend((ids as u16).to_le_bytes()); // записей с номерами
    };
    let mut s = Vec::new();
    // Уровень 1: типы.
    header(&mut s, types.len());
    let mut offset = level2_start;
    for (i, &t) in types.iter().enumerate() {
        s.extend(t.to_le_bytes());
        s.extend((offset as u32 | 0x8000_0000).to_le_bytes());
        offset += level2[i];
    }
    // Уровень 2: номера внутри типа.
    let mut leaf = 0;
    for &t in &types {
        let ids: Vec<u32> = resources.iter().filter(|r| r.0 == t).map(|r| r.1).collect();
        header(&mut s, ids.len());
        for id in ids {
            s.extend(id.to_le_bytes());
            s.extend(((level3_start + leaf * directory(1)) as u32 | 0x8000_0000).to_le_bytes());
            leaf += 1;
        }
    }
    // Уровень 3: один язык — нейтральный.
    for i in 0..resources.len() {
        header(&mut s, 1);
        s.extend(0u32.to_le_bytes());
        s.extend(((entries_start + i * 16) as u32).to_le_bytes());
    }
    // Записи данных: смещение данных (поправит компоновщик до RVA), размер, кодовая страница.
    let mut relocations = Vec::new();
    let mut data_offset = data_start;
    for r in resources.iter() {
        relocations.push(s.len() as u32);
        s.extend((data_offset as u32).to_le_bytes());
        s.extend((r.2.len() as u32).to_le_bytes());
        s.extend([0u8; 8]);
        data_offset = (data_offset + r.2.len()).div_ceil(8) * 8;
    }
    s.resize(data_start, 0);
    for r in resources.iter() {
        s.extend_from_slice(&r.2);
        s.resize(s.len().div_ceil(8) * 8, 0);
    }
    (s, relocations)
}

/// Объектный файл COFF x64: один раздел .rsrc, поправки IMAGE_REL_AMD64_ADDR32NB к его символу.
fn coff(section: &[u8], relocations: &[u32]) -> Vec<u8> {
    let raw_at = 20 + 40;
    let relocations_at = raw_at + section.len();
    let symbols_at = relocations_at + relocations.len() * 10;
    let mut o = Vec::new();
    o.extend(0x8664u16.to_le_bytes()); // x64
    o.extend(1u16.to_le_bytes()); // разделов
    o.extend(0u32.to_le_bytes()); // время
    o.extend((symbols_at as u32).to_le_bytes());
    o.extend(1u32.to_le_bytes()); // символов
    o.extend(0u16.to_le_bytes()); // необязательного заголовка нет
    o.extend(0u16.to_le_bytes());
    o.extend(*b".rsrc\0\0\0");
    o.extend(0u32.to_le_bytes());
    o.extend(0u32.to_le_bytes());
    o.extend((section.len() as u32).to_le_bytes());
    o.extend((raw_at as u32).to_le_bytes());
    o.extend((relocations_at as u32).to_le_bytes());
    o.extend(0u32.to_le_bytes());
    o.extend((relocations.len() as u16).to_le_bytes());
    o.extend(0u16.to_le_bytes());
    // Данные, только чтение, выравнивание 4 байта.
    o.extend(0x4030_0040u32.to_le_bytes());
    o.extend_from_slice(section);
    for &at in relocations {
        o.extend(at.to_le_bytes());
        o.extend(0u32.to_le_bytes()); // символ 0 — раздел .rsrc
        o.extend(3u16.to_le_bytes()); // IMAGE_REL_AMD64_ADDR32NB
    }
    o.extend(*b".rsrc\0\0\0");
    o.extend(0u32.to_le_bytes()); // значение
    o.extend(1i16.to_le_bytes()); // раздел 1
    o.extend(0u16.to_le_bytes()); // тип
    o.push(3); // IMAGE_SYM_CLASS_STATIC
    o.push(0); // дополнительных записей
    o.extend(4u32.to_le_bytes()); // таблица строк пуста
    o
}

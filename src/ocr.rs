//! Windows OCR (часть 3; P 1446–1497). Создаётся по требованию и отпускается через 60 с (часть 4).
//!
//! На урезанных сборках Windows распознавания может не быть совсем (нет класса WinRT или ни одного
//! языка OCR): `supported()` тогда `false`, `new()` — `Err`, без паники; остальная программа работает.
//! Ожидание `RecognizeAsync` — через обработчик `Completed` и событие (канал), 5 с, затем `Cancel`.

use std::sync::mpsc;
use std::time::Duration;

use windows::core::HSTRING;
use windows::Foundation::{AsyncOperationCompletedHandler, AsyncStatus, IAsyncOperation};
use windows::Globalization::Language;
use windows::Graphics::Imaging::{BitmapAlphaMode, BitmapPixelFormat, SoftwareBitmap};
use windows::Media::Ocr::{OcrEngine, OcrResult};
use windows::Storage::Streams::DataWriter;

use crate::types::Rect;
use crate::win::Bitmap;

/// Сколько ждать распознавания (Python: `asyncio.wait_for(…, 5.0)`).
const WAIT: Duration = Duration::from_secs(5);
/// Языки движков по порядку (P 1459).
const LANGUAGES: [&str; 2] = ["ru", "en-US"];

/// Ждать завершения операции: обработчик `Completed` сигналит в канал; нет ответа за `WAIT` —
/// `Cancel` и `Err`. Обработчик может сработать и позже — канал тогда просто никто не читает.
fn wait(operation: &IAsyncOperation<OcrResult>) -> Result<OcrResult, ()> {
    let (done, finished) = mpsc::sync_channel::<()>(1);
    let handler = AsyncOperationCompletedHandler::new(move |_, _| {
        let _ = done.try_send(());
        Ok(())
    });
    // Уже завершённая операция вызывает обработчик сразу, внутри SetCompleted.
    operation.SetCompleted(&handler).map_err(|_| ())?;
    if finished.recv_timeout(WAIT).is_err() {
        let _ = operation.Cancel();
        return Err(());
    }
    match operation.Status() {
        Ok(AsyncStatus::Completed) => operation.GetResults().map_err(|_| ()),
        _ => Err(()),
    }
}

fn engine_for(tag: &str) -> Option<OcrEngine> {
    let language = Language::CreateLanguage(&HSTRING::from(tag)).ok()?;
    if !OcrEngine::IsLanguageSupported(&language).unwrap_or(false) {
        return None;
    }
    // TryCreate… отдаёт NULL, если не вышло, — это `Err` -> `None`.
    OcrEngine::TryCreateFromLanguage(&language).ok()
}

#[derive(Clone, Debug)]
pub struct Word {
    pub text: String,
    pub rect: Rect,
}

/// Движки ru, en-US или языка пользователя. Живёт в потоке tracker (поле `Worker::ocr`).
pub struct Ocr {
    engines: Vec<(&'static str, OcrEngine)>, // "ru", "en-US", иначе "user" или "other"; не пуст
    max_side: u32,                           // OcrEngine::MaxImageDimension
}

impl Ocr {
    /// Дёшево, без создания движков.
    /// `true`, если в Windows есть распознавание и хотя бы один установленный язык OCR.
    /// Урезанная сборка без класса WinRT или без языков — `false` (не паника).
    pub fn supported() -> bool {
        match OcrEngine::AvailableRecognizerLanguages() {
            Ok(languages) => languages.Size().map(|n| n > 0).unwrap_or(false),
            Err(_) => false,
        }
    }

    /// Движки ru и en-US (что установлено); иначе — языков пользователя; иначе — первый
    /// установленный язык OCR. Ни одного — `Err`.
    pub fn new() -> Result<Ocr, ()> {
        let mut engines: Vec<(&'static str, OcrEngine)> =
            LANGUAGES.iter().filter_map(|&tag| engine_for(tag).map(|e| (tag, e))).collect();
        if engines.is_empty() {
            if let Ok(engine) = OcrEngine::TryCreateFromUserProfileLanguages() {
                engines.push(("user", engine));
            }
        }
        if engines.is_empty() {
            // Дополнение к Python: установлен только «чужой» язык (не ru/en-US и не язык профиля).
            let first = OcrEngine::AvailableRecognizerLanguages()
                .and_then(|languages| languages.GetAt(0))
                .and_then(|language| OcrEngine::TryCreateFromLanguage(&language));
            if let Ok(engine) = first {
                engines.push(("other", engine));
            }
        }
        if engines.is_empty() {
            return Err(());
        }
        let max_side = OcrEngine::MaxImageDimension().map_err(|_| ())?;
        Ok(Ocr { engines, max_side })
    }

    fn engine(&self, tag: &str) -> Option<&OcrEngine> {
        self.engines.iter().find(|(t, _)| *t == tag).map(|(_, e)| e)
    }

    /// Ожидание до 5 с, затем Cancel.
    /// Слова в координатах битмапа, в порядке чтения. Пустой битмап или сторона больше
    /// `MaxImageDimension` — пустой список; длина `bgra` не `width × height × 4` — `Err`.
    pub fn words(&self, bmp: &Bitmap, cyrillic: bool) -> Result<Vec<Word>, ()> {
        let (width, height) = (bmp.width, bmp.height);
        if bmp.bgra.is_empty() || width <= 0 || height <= 0 || width as u32 > self.max_side || height as u32 > self.max_side
        {
            return Ok(Vec::new());
        }
        if bmp.bgra.len() != width as usize * height as usize * 4 {
            return Err(());
        }
        let engine = self
            .engine(if cyrillic { "ru" } else { "en-US" })
            .or_else(|| self.engine("en-US"))
            .or_else(|| self.engines.first().map(|(_, e)| e))
            .ok_or(())?;
        let writer = DataWriter::new().map_err(|_| ())?;
        writer.WriteBytes(&bmp.bgra).map_err(|_| ())?;
        let buffer = writer.DetachBuffer().map_err(|_| ())?;
        let bitmap = SoftwareBitmap::CreateCopyWithAlphaFromBuffer(
            &buffer,
            BitmapPixelFormat::Bgra8,
            width,
            height,
            BitmapAlphaMode::Premultiplied,
        )
        .map_err(|_| ())?;
        let operation = engine.RecognizeAsync(&bitmap).map_err(|_| ())?;
        let result = wait(&operation)?;
        let mut found = Vec::new();
        let lines = result.Lines().map_err(|_| ())?;
        for i in 0..lines.Size().map_err(|_| ())? {
            let words = lines.GetAt(i).and_then(|line| line.Words()).map_err(|_| ())?;
            for j in 0..words.Size().map_err(|_| ())? {
                let word = words.GetAt(j).map_err(|_| ())?;
                let text = word.Text().map_err(|_| ())?;
                let b = word.BoundingRect().map_err(|_| ())?;
                // Python: int(x), int(x + width) — к нулю, сумма в double.
                let (x, y) = (f64::from(b.X), f64::from(b.Y));
                found.push(Word {
                    text: String::from_utf16_lossy(text.as_wide()),
                    rect: Rect {
                        l: x as i32,
                        t: y as i32,
                        r: (x + f64::from(b.Width)) as i32,
                        b: (y + f64::from(b.Height)) as i32,
                    },
                });
            }
        }
        let _ = bitmap.Close();
        Ok(found)
    }
}

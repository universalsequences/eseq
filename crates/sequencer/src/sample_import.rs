use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use crate::sample_db::SampleDb;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StagedSampleStatus {
    Ready,
    Duplicate,
    Error(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StagedSample {
    pub source_path: PathBuf,
    pub hash: Option<String>,
    pub title: String,
    pub tags: Vec<String>,
    pub status: StagedSampleStatus,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportSummary {
    pub imported: usize,
    pub duplicates: usize,
    pub failed: usize,
}

pub(crate) struct DecodedAudio {
    pub(crate) sample_rate: u32,
    pub(crate) channels: u16,
    pub(crate) samples: Vec<f32>,
}

const AUDIO_EXTENSIONS: &[&str] = &["wav", "aif", "aiff", "mp3", "flac"];

pub fn stage_paths(paths: &[PathBuf], db: &SampleDb) -> Vec<StagedSample> {
    let mut files = collect_audio_files(paths);
    files.sort();
    files.dedup();
    files.into_iter().map(|path| stage_file(path, db)).collect()
}

pub fn collect_audio_files(paths: &[PathBuf]) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for path in paths {
        collect_audio_path(path, &mut out);
    }
    out
}

pub fn import_staged_samples(
    staged: &[StagedSample],
    batch_tags: &[String],
    sample_dir: &Path,
    db: &mut SampleDb,
) -> ImportSummary {
    let mut summary = ImportSummary {
        imported: 0,
        duplicates: 0,
        failed: 0,
    };
    let _ = fs::create_dir_all(sample_dir);
    for sample in staged {
        match &sample.status {
            StagedSampleStatus::Duplicate => {
                summary.duplicates += 1;
                continue;
            }
            StagedSampleStatus::Error(_) => {
                summary.failed += 1;
                continue;
            }
            StagedSampleStatus::Ready => {}
        }
        let Some(hash) = &sample.hash else {
            summary.failed += 1;
            continue;
        };
        match import_one(sample, hash, batch_tags, sample_dir, db) {
            Ok(true) => summary.imported += 1,
            Ok(false) => summary.duplicates += 1,
            Err(error) => {
                eprintln!(
                    "sample import failed for {}: {error}",
                    sample.source_path.display()
                );
                summary.failed += 1;
            }
        }
    }
    summary
}

fn import_one(
    sample: &StagedSample,
    hash: &str,
    batch_tags: &[String],
    sample_dir: &Path,
    db: &mut SampleDb,
) -> Result<bool, String> {
    if db
        .contains_sample(hash)
        .map_err(|error| format!("failed to query duplicate sample: {error}"))?
    {
        return Ok(false);
    }
    let decoded = decode_audio_file(&sample.source_path)?;
    let dest = sample_dir.join(format!("{hash}.wav"));
    let tmp = sample_dir.join(format!("{hash}.wav.tmp"));
    write_wav(&tmp, &decoded)?;
    fs::rename(&tmp, &dest).map_err(|error| {
        let _ = fs::remove_file(&tmp);
        format!(
            "failed to move {} to {}: {error}",
            tmp.display(),
            dest.display()
        )
    })?;
    let mut tags = batch_tags.to_vec();
    tags.extend(sample.tags.clone());
    tags = normalize_tags(&tags);
    let inserted = db
        .insert_sample_with_tags(hash, Some(&sample.title), &tags)
        .map_err(|error| {
            let _ = fs::remove_file(&dest);
            format!("failed to insert sample row: {error}")
        })?;
    if !inserted {
        let _ = fs::remove_file(&dest);
        return Ok(false);
    }
    Ok(true)
}

fn collect_audio_path(path: &Path, out: &mut Vec<PathBuf>) {
    if is_hidden_path(path) {
        return;
    }
    if path.is_file() {
        if is_supported_audio_file(path) {
            out.push(path.to_path_buf());
        }
        return;
    }
    if !path.is_dir() {
        return;
    }
    let Ok(entries) = fs::read_dir(path) else {
        return;
    };
    for entry in entries.flatten() {
        collect_audio_path(&entry.path(), out);
    }
}

fn is_hidden_path(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.starts_with('.'))
}

fn is_supported_audio_file(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| {
            let ext = ext.to_ascii_lowercase();
            AUDIO_EXTENSIONS.contains(&ext.as_str())
        })
        .unwrap_or(false)
}

fn stage_file(path: PathBuf, db: &SampleDb) -> StagedSample {
    let title = default_title_for_path(&path);
    let bytes = match fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) => {
            return StagedSample {
                source_path: path,
                hash: None,
                title,
                tags: Vec::new(),
                status: StagedSampleStatus::Error(format!("read failed: {error}")),
            };
        }
    };
    let hash = hex_sha256(&bytes);
    let status = match db.contains_sample(&hash) {
        Ok(true) => StagedSampleStatus::Duplicate,
        Ok(false) => match decode_audio_file(&path) {
            Ok(_) => StagedSampleStatus::Ready,
            Err(error) => StagedSampleStatus::Error(error),
        },
        Err(error) => StagedSampleStatus::Error(format!("db query failed: {error}")),
    };
    StagedSample {
        source_path: path,
        hash: Some(hash),
        title,
        tags: Vec::new(),
        status,
    }
}

pub fn normalize_tags(tags: &[String]) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for tag in tags {
        let tag = tag.trim();
        if tag.is_empty() {
            continue;
        }
        let key = tag.to_lowercase();
        if seen.insert(key) {
            out.push(tag.to_string());
        }
    }
    out
}

fn default_title_for_path(path: &Path) -> String {
    path.file_stem()
        .and_then(|stem| stem.to_str())
        .map(|stem| stem.replace(['_', '-'], " "))
        .map(|title| title.split_whitespace().collect::<Vec<_>>().join(" "))
        .filter(|title| !title.is_empty())
        .unwrap_or_else(|| "Untitled sample".to_string())
}

fn hex_sha256(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut out = String::with_capacity(digest.len() * 2);
    for byte in digest {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

pub(crate) fn decode_audio_file(path: &Path) -> Result<DecodedAudio, String> {
    let ext = path
        .extension()
        .and_then(|ext| ext.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    if ext == "wav" {
        let decoded = eseqlisp::audio::sample::load_wav_file(path)?;
        return Ok(DecodedAudio {
            sample_rate: decoded.sample_rate,
            channels: decoded.channels,
            samples: decoded.samples,
        });
    }
    decode_with_symphonia(path)
}

fn decode_with_symphonia(path: &Path) -> Result<DecodedAudio, String> {
    use symphonia::core::codecs::{DecoderOptions, CODEC_TYPE_NULL};
    use symphonia::core::errors::Error as SymphoniaError;
    use symphonia::core::formats::FormatOptions;
    use symphonia::core::io::MediaSourceStream;
    use symphonia::core::meta::MetadataOptions;
    use symphonia::core::probe::Hint;
    use symphonia::default::{get_codecs, get_probe};

    let file = fs::File::open(path)
        .map_err(|error| format!("failed to open {}: {error}", path.display()))?;
    let mss = MediaSourceStream::new(Box::new(file), Default::default());
    let mut hint = Hint::new();
    if let Some(ext) = path.extension().and_then(|ext| ext.to_str()) {
        hint.with_extension(ext);
    }
    let probed = get_probe()
        .format(
            &hint,
            mss,
            &FormatOptions::default(),
            &MetadataOptions::default(),
        )
        .map_err(|error| format!("failed to probe {}: {error}", path.display()))?;
    let mut format = probed.format;
    let track = format
        .tracks()
        .iter()
        .find(|track| track.codec_params.codec != CODEC_TYPE_NULL)
        .ok_or_else(|| "no supported audio track".to_string())?;
    let track_id = track.id;
    let sample_rate = track
        .codec_params
        .sample_rate
        .ok_or_else(|| "audio track has no sample rate".to_string())?;
    let channels = track
        .codec_params
        .channels
        .ok_or_else(|| "audio track has no channel layout".to_string())?
        .count() as u16;
    let mut decoder = get_codecs()
        .make(&track.codec_params, &DecoderOptions::default())
        .map_err(|error| format!("failed to create decoder: {error}"))?;
    let mut samples = Vec::new();
    loop {
        let packet = match format.next_packet() {
            Ok(packet) => packet,
            Err(SymphoniaError::IoError(error))
                if error.kind() == std::io::ErrorKind::UnexpectedEof =>
            {
                break
            }
            Err(SymphoniaError::ResetRequired) => {
                return Err("decoder reset required".to_string());
            }
            Err(error) => return Err(format!("failed to read packet: {error}")),
        };
        if packet.track_id() != track_id {
            continue;
        }
        let decoded = match decoder.decode(&packet) {
            Ok(decoded) => decoded,
            Err(SymphoniaError::DecodeError(_)) => continue,
            Err(error) => return Err(format!("failed to decode packet: {error}")),
        };
        append_interleaved_f32(&decoded, &mut samples);
    }
    if samples.is_empty() {
        return Err("decoded audio was empty".to_string());
    }
    let expected_channels = channels.max(1) as usize;
    let remainder = samples.len() % expected_channels;
    if remainder != 0 {
        samples.truncate(samples.len() - remainder);
    }
    Ok(DecodedAudio {
        sample_rate,
        channels: channels.max(1),
        samples,
    })
}

fn append_interleaved_f32(
    decoded: &symphonia::core::audio::AudioBufferRef<'_>,
    out: &mut Vec<f32>,
) {
    use symphonia::core::audio::AudioBufferRef;
    use symphonia::core::conv::FromSample;

    match decoded {
        AudioBufferRef::F32(buf) => append_planar(buf, out, |sample| sample),
        AudioBufferRef::F64(buf) => append_planar(buf, out, |sample| sample as f32),
        AudioBufferRef::U8(buf) => append_planar(buf, out, f32::from_sample),
        AudioBufferRef::U16(buf) => append_planar(buf, out, f32::from_sample),
        AudioBufferRef::U24(buf) => append_planar(buf, out, f32::from_sample),
        AudioBufferRef::U32(buf) => append_planar(buf, out, f32::from_sample),
        AudioBufferRef::S8(buf) => append_planar(buf, out, f32::from_sample),
        AudioBufferRef::S16(buf) => append_planar(buf, out, f32::from_sample),
        AudioBufferRef::S24(buf) => append_planar(buf, out, f32::from_sample),
        AudioBufferRef::S32(buf) => append_planar(buf, out, f32::from_sample),
    }
}

fn append_planar<S, F>(buf: &symphonia::core::audio::AudioBuffer<S>, out: &mut Vec<f32>, convert: F)
where
    S: symphonia::core::sample::Sample,
    F: Fn(S) -> f32,
{
    use symphonia::core::audio::Signal;

    let channels = buf.spec().channels.count();
    let frames = buf.frames();
    out.reserve(channels * frames);
    for frame in 0..frames {
        for channel in 0..channels {
            out.push(convert(buf.chan(channel)[frame]));
        }
    }
}

fn write_wav(path: &Path, decoded: &DecodedAudio) -> Result<(), String> {
    let spec = hound::WavSpec {
        channels: decoded.channels,
        sample_rate: decoded.sample_rate,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut writer = hound::WavWriter::create(path, spec)
        .map_err(|error| format!("failed to create {}: {error}", path.display()))?;
    for sample in &decoded.samples {
        let scaled = (sample.clamp(-1.0, 1.0) * i16::MAX as f32).round() as i16;
        writer
            .write_sample(scaled)
            .map_err(|error| format!("failed to write {}: {error}", path.display()))?;
    }
    writer
        .finalize()
        .map_err(|error| format!("failed to finalize {}: {error}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::params;

    fn unique_temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "eseq-sample-import-{name}-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn write_test_wav(path: &Path) {
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate: 44_100,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut writer = hound::WavWriter::create(path, spec).unwrap();
        writer.write_sample(0i16).unwrap();
        writer.write_sample(1200i16).unwrap();
        writer.finalize().unwrap();
    }

    #[test]
    fn collect_audio_files_recurses_and_ignores_hidden_and_unsupported() {
        let dir = unique_temp_dir("collect");
        let nested = dir.join("nested");
        let hidden = dir.join(".hidden");
        fs::create_dir_all(&nested).unwrap();
        fs::create_dir_all(&hidden).unwrap();
        fs::write(dir.join("root.wav"), b"not decoded here").unwrap();
        fs::write(nested.join("bass.flac"), b"not decoded here").unwrap();
        fs::write(nested.join("notes.txt"), b"ignore").unwrap();
        fs::write(hidden.join("secret.wav"), b"ignore").unwrap();

        let mut files = collect_audio_files(&[dir.clone()]);
        files.sort();
        assert_eq!(files, vec![nested.join("bass.flac"), dir.join("root.wav")]);
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn import_wav_creates_canonical_file_db_row_title_and_tags() {
        let dir = unique_temp_dir("import");
        let source = dir.join("Deep_Kick.wav");
        write_test_wav(&source);
        let mut db = SampleDb::open_in_memory().unwrap();
        let staged = stage_paths(&[source], &db);
        assert_eq!(staged.len(), 1);
        assert!(matches!(staged[0].status, StagedSampleStatus::Ready));

        let summary = import_staged_samples(
            &staged,
            &["drum".to_string(), "kick".to_string()],
            &dir.join("samples"),
            &mut db,
        );
        assert_eq!(summary.imported, 1);
        let hash = staged[0].hash.as_ref().unwrap();
        assert!(dir.join("samples").join(format!("{hash}.wav")).is_file());
        assert_eq!(
            db.title_for_hash(hash).unwrap(),
            Some("Deep Kick".to_string())
        );
        assert_eq!(db.tags_for(hash).unwrap(), vec!["drum", "kick"]);
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn duplicate_hash_is_skipped_without_retagging() {
        let dir = unique_temp_dir("duplicate");
        let source = dir.join("hat.wav");
        write_test_wav(&source);
        let mut db = SampleDb::open_in_memory().unwrap();
        let staged = stage_paths(&[source], &db);
        let hash = staged[0].hash.as_ref().unwrap().clone();
        db.connection()
            .execute(
                "INSERT INTO samples(hash, title) VALUES (?, 'Existing')",
                params![hash],
            )
            .unwrap();

        let restaged = stage_paths(&[dir.join("hat.wav")], &db);
        assert!(matches!(restaged[0].status, StagedSampleStatus::Duplicate));
        let summary = import_staged_samples(
            &restaged,
            &["new-tag".to_string()],
            &dir.join("samples"),
            &mut db,
        );
        assert_eq!(summary.duplicates, 1);
        assert!(db.tags_for(&hash).unwrap().is_empty());
        let _ = fs::remove_dir_all(dir);
    }
}

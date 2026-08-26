//! Shared offline-model manifest, verified downloader, and ONNX Runtime bootstrap.
//!
//! All offline ONNX consumers share one model root (`{app_data_dir}/rapidocr-models`)
//! and one process-wide ONNX Runtime instance.

use std::collections::{HashMap, HashSet};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex as StdMutex};
use std::time::Duration;

#[cfg(target_os = "macos")]
use flate2::read::GzDecoder;
use reqwest::header::{ACCEPT_ENCODING, CONTENT_RANGE, RANGE, USER_AGENT};
use serde::Serialize;
use sha2::{Digest, Sha256};
use tauri::{AppHandle, Emitter, Manager};
use tokio::io::AsyncWriteExt;
use tokio::sync::{Mutex, OnceCell};

#[cfg(target_os = "windows")]
use std::os::windows::ffi::OsStrExt;
#[cfg(target_os = "windows")]
use windows::{
    core::PCWSTR,
    Win32::System::LibraryLoader::{
        LoadLibraryExW, LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR, LOAD_LIBRARY_SEARCH_SYSTEM32,
    },
};

const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(5 * 60);
const MAX_ATTEMPTS: u8 = 3;
/// Per-chunk progress emits are throttled to at most one per interval…
const PROGRESS_EMIT_INTERVAL: Duration = Duration::from_millis(250);
/// …or one per this many bytes accumulated, whichever comes first.
const PROGRESS_EMIT_BYTES: u64 = 1024 * 1024;
const DOWNLOAD_USER_AGENT: &str =
    "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/124.0 Safari/537.36 Kivio/1.0";

#[cfg(target_os = "macos")]
pub const ONNX_RUNTIME_FILE_NAME: &str = "libonnxruntime.dylib";
#[cfg(target_os = "windows")]
pub const ONNX_RUNTIME_FILE_NAME: &str = "onnxruntime.dll";
#[cfg(target_os = "windows")]
const PROVIDERS_SHARED_NAME: &str = "onnxruntime_providers_shared.dll";

#[derive(Debug, Clone, Copy)]
enum ModelSource {
    Direct {
        url: &'static str,
        size: u64,
        sha256: &'static str,
    },
    Archive {
        cache_key: &'static str,
        url: &'static str,
        size: u64,
        sha256: &'static str,
        entry_suffix: &'static str,
    },
}

impl ModelSource {
    fn key(self) -> &'static str {
        match self {
            Self::Direct { url, .. } => url,
            Self::Archive { cache_key, .. } => cache_key,
        }
    }

    fn url(self) -> &'static str {
        match self {
            Self::Direct { url, .. } | Self::Archive { url, .. } => url,
        }
    }

    fn size(self) -> u64 {
        match self {
            Self::Direct { size, .. } | Self::Archive { size, .. } => size,
        }
    }

    fn sha256(self) -> &'static str {
        match self {
            Self::Direct { sha256, .. } | Self::Archive { sha256, .. } => sha256,
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct ModelFile {
    component_id: &'static str,
    relative_path: &'static str,
    installed_size: u64,
    installed_sha256: &'static str,
    source: ModelSource,
}

// 离线 OCR 双档位:
// - Standard = PP-OCRv5 mobile(~20MB,快),文件落模型目录根(det/rec/keys.onnx)。
// - High     = PP-OCRv6 medium(~139MB,精度高),文件落 `high/` 子目录。
//   保留 `high/` 布局是为了让早前装过 high 档的用户无需重新下载 ~139MB。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OcrModelTier {
    Standard,
    High,
}

impl OcrModelTier {
    pub fn parse(value: &str) -> Self {
        if value == "high" {
            Self::High
        } else {
            Self::Standard
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Standard => "standard",
            Self::High => "high",
        }
    }
}

// ---- Standard 档:PP-OCRv5 mobile(GreatV/oar-ocr v0.3.0 release),模型目录根 ----
const STANDARD_DET: ModelFile = ModelFile {
    component_id: "rapidocr-standard",
    relative_path: "det.onnx",
    installed_size: 4_826_518,
    installed_sha256: "1eb7b4f7ab657ebd1c66d5f79bca7497f29768a2e3c15e52daecbba1a8e4a039",
    source: ModelSource::Direct {
        url: "https://github.com/GreatV/oar-ocr/releases/download/v0.3.0/pp-ocrv5_mobile_det.onnx",
        size: 4_826_518,
        sha256: "1eb7b4f7ab657ebd1c66d5f79bca7497f29768a2e3c15e52daecbba1a8e4a039",
    },
};
const STANDARD_REC: ModelFile = ModelFile {
    component_id: "rapidocr-standard",
    relative_path: "rec.onnx",
    installed_size: 16_562_373,
    installed_sha256: "243a0f06d826761323e9045e9b113ab2c191c3aa50565585e628300b8eda0224",
    source: ModelSource::Direct {
        url: "https://github.com/GreatV/oar-ocr/releases/download/v0.3.0/pp-ocrv5_mobile_rec.onnx",
        size: 16_562_373,
        sha256: "243a0f06d826761323e9045e9b113ab2c191c3aa50565585e628300b8eda0224",
    },
};
const STANDARD_KEYS: ModelFile = ModelFile {
    component_id: "rapidocr-standard",
    relative_path: "keys.txt",
    installed_size: 74_012,
    installed_sha256: "d1979e9f794c464c0d2e0b70a7fe14dd978e9dc644c0e71f14158cdf8342af1b",
    source: ModelSource::Direct {
        url: "https://github.com/GreatV/oar-ocr/releases/download/v0.3.0/ppocrv5_dict.txt",
        size: 74_012,
        sha256: "d1979e9f794c464c0d2e0b70a7fe14dd978e9dc644c0e71f14158cdf8342af1b",
    },
};
// ---- High 档:PP-OCRv6 medium(ModelScope),`high/` 子目录 ----
const HIGH_DET: ModelFile = ModelFile {
    component_id: "rapidocr-high",
    relative_path: "high/det.onnx",
    installed_size: 62_032_837,
    installed_sha256: "eb13b44b25bb36f89528b68720af8a61d9cf381176107f465db1757b65d086e1",
    source: ModelSource::Direct {
        url: "https://www.modelscope.cn/api/v1/models/greatv/oar-ocr/repo?Revision=master&FilePath=pp-ocrv6_medium_det.onnx",
        size: 62_032_837,
        sha256: "eb13b44b25bb36f89528b68720af8a61d9cf381176107f465db1757b65d086e1",
    },
};
const HIGH_REC: ModelFile = ModelFile {
    component_id: "rapidocr-high",
    relative_path: "high/rec.onnx",
    installed_size: 76_554_979,
    installed_sha256: "9c09abf0957f7968c7586464b7397b84ad2387a0497a351af40e9acc71b673ba",
    source: ModelSource::Direct {
        url: "https://www.modelscope.cn/api/v1/models/greatv/oar-ocr/repo?Revision=master&FilePath=pp-ocrv6_medium_rec.onnx",
        size: 76_554_979,
        sha256: "9c09abf0957f7968c7586464b7397b84ad2387a0497a351af40e9acc71b673ba",
    },
};
const HIGH_KEYS: ModelFile = ModelFile {
    component_id: "rapidocr-high",
    relative_path: "high/keys.txt",
    installed_size: 74_947,
    installed_sha256: "b5f2bfe2bdd9448429e3e82b51c789775d9b42f2403d082b00662eb77e401c5d",
    source: ModelSource::Direct {
        url: "https://www.modelscope.cn/api/v1/models/greatv/oar-ocr/repo?Revision=master&FilePath=ppocrv6_dict.txt",
        size: 74_947,
        sha256: "b5f2bfe2bdd9448429e3e82b51c789775d9b42f2403d082b00662eb77e401c5d",
    },
};
#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
const RUNTIME_SOURCE: ModelSource = ModelSource::Archive {
    cache_key: "onnxruntime-osx-arm64-1.24.4.tgz",
    url: "https://github.com/microsoft/onnxruntime/releases/download/v1.24.4/onnxruntime-osx-arm64-1.24.4.tgz",
    size: 30_937_282,
    sha256: "93787795f47e1eee369182e43ed51b9e5da0878ab0346aecf4258979b8bba989",
    entry_suffix: "lib/libonnxruntime.dylib",
};
#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
const RUNTIME: ModelFile = ModelFile {
    component_id: "onnx-runtime",
    relative_path: ONNX_RUNTIME_FILE_NAME,
    installed_size: 35_418_600,
    installed_sha256: "872533f130f1839a5bc01788ddb4f75c83a189763441ba1178788ed965449289",
    source: RUNTIME_SOURCE,
};

// ONNX Runtime stopped publishing Intel macOS packages after 1.23.x. The Rust
// binding uses API 23, which remains compatible with 1.24 runtimes on arm64/Windows.
#[cfg(all(target_os = "macos", target_arch = "x86_64"))]
const RUNTIME_SOURCE: ModelSource = ModelSource::Archive {
    cache_key: "onnxruntime-osx-x86_64-1.23.2.tgz",
    url: "https://github.com/microsoft/onnxruntime/releases/download/v1.23.2/onnxruntime-osx-x86_64-1.23.2.tgz",
    size: 11_676_322,
    sha256: "d10359e16347b57d9959f7e80a225a5b4a66ed7d7e007274a15cae86836485a6",
    entry_suffix: "lib/libonnxruntime.1.23.2.dylib",
};
#[cfg(all(target_os = "macos", target_arch = "x86_64"))]
const RUNTIME: ModelFile = ModelFile {
    component_id: "onnx-runtime",
    relative_path: ONNX_RUNTIME_FILE_NAME,
    installed_size: 39_742_608,
    installed_sha256: "8c9c78de65ea3786f987c0d980e9c1b13a3a5fbc6b3e2965ba05b450e6e4c054",
    source: RUNTIME_SOURCE,
};

#[cfg(target_os = "windows")]
const RUNTIME_SOURCE: ModelSource = ModelSource::Archive {
    cache_key: "onnxruntime-win-x64-1.24.4.zip",
    url: "https://github.com/microsoft/onnxruntime/releases/download/v1.24.4/onnxruntime-win-x64-1.24.4.zip",
    size: 74_442_783,
    sha256: "d2319fddfb6ea4db99ccc4b60c85c517bcd855721f5daa6a06d40d7cb2ee2357",
    entry_suffix: "lib/onnxruntime.dll",
};
#[cfg(target_os = "windows")]
const RUNTIME: ModelFile = ModelFile {
    component_id: "onnx-runtime",
    relative_path: ONNX_RUNTIME_FILE_NAME,
    installed_size: 14_203_464,
    installed_sha256: "b95efb2113b603bbbf3f191061c5516a871ed546893c820e4f3b7b6c358dbf2a",
    source: RUNTIME_SOURCE,
};
#[cfg(target_os = "windows")]
const PROVIDERS_SHARED: ModelFile = ModelFile {
    component_id: "onnx-runtime",
    relative_path: PROVIDERS_SHARED_NAME,
    installed_size: 22_088,
    installed_sha256: "f2540b89707b47895c2a732bfd04e34a695c580d22301ef44c0f01f09b001673",
    source: ModelSource::Archive {
        cache_key: "onnxruntime-win-x64-1.24.4.zip",
        url: "https://github.com/microsoft/onnxruntime/releases/download/v1.24.4/onnxruntime-win-x64-1.24.4.zip",
        size: 74_442_783,
        sha256: "d2319fddfb6ea4db99ccc4b60c85c517bcd855721f5daa6a06d40d7cb2ee2357",
        entry_suffix: "lib/onnxruntime_providers_shared.dll",
    },
};

fn runtime_files() -> Vec<ModelFile> {
    #[cfg(target_os = "windows")]
    {
        vec![RUNTIME, PROVIDERS_SHARED]
    }
    #[cfg(not(target_os = "windows"))]
    {
        vec![RUNTIME]
    }
}

fn ocr_files(tier: OcrModelTier) -> Vec<ModelFile> {
    match tier {
        OcrModelTier::Standard => vec![STANDARD_DET, STANDARD_REC, STANDARD_KEYS],
        OcrModelTier::High => vec![HIGH_DET, HIGH_REC, HIGH_KEYS],
    }
}

fn rapidocr_pack(tier: OcrModelTier) -> Vec<ModelFile> {
    let mut files = runtime_files();
    files.extend(ocr_files(tier));
    files
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OfflineModelFileStatus {
    pub component_id: String,
    pub file_name: String,
    pub installed_bytes: u64,
    pub download_bytes: u64,
    pub ready: bool,
    pub state: OfflineModelFileState,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum OfflineModelFileState {
    Ready,
    Missing,
    Invalid,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReplaceTranslationPackStatus {
    pub tier: String,
    pub ready: bool,
    pub total_bytes: u64,
    pub ready_bytes: u64,
    pub missing_bytes: u64,
    pub model_dir: Option<String>,
    pub files: Vec<OfflineModelFileStatus>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OfflineModelInstallResult {
    pub success: bool,
    pub message: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OfflineModelProgress {
    /// Which install pack this event belongs to. The settings panels filter on
    /// this so a plain RapidOCR install does not drive the replace-translation
    /// pack UI (both packs share the same event name).
    pub pack: OfflineModelPack,
    pub component_id: String,
    pub file_name: String,
    pub downloaded_bytes: u64,
    pub file_total_bytes: u64,
    pub overall_downloaded_bytes: u64,
    pub overall_total_bytes: u64,
    pub attempt: u8,
    pub state: OfflineModelProgressState,
    pub error: Option<String>,
}

/// Frontend contract: serialized as `"rapidocr"` / `"replace_translation"`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum OfflineModelPack {
    Rapidocr,
    ReplaceTranslation,
}

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum OfflineModelProgressState {
    Downloading,
    Retrying,
    Verifying,
    Extracting,
    Completed,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ValidationState {
    Ready,
    Missing,
    Invalid,
}

pub struct OfflineModelManager {
    app: Option<AppHandle>,
    http: reqwest::Client,
    install_lock: Mutex<()>,
    model_dir_override: Option<PathBuf>,
    verified_files: StdMutex<HashMap<PathBuf, (u64, Option<std::time::SystemTime>)>>,
}

impl OfflineModelManager {
    pub fn new(app: &AppHandle, http: reqwest::Client) -> Arc<Self> {
        Arc::new(Self {
            app: Some(app.clone()),
            http,
            install_lock: Mutex::new(()),
            model_dir_override: None,
            verified_files: StdMutex::new(HashMap::new()),
        })
    }

    pub fn headless(http: reqwest::Client) -> Arc<Self> {
        Arc::new(Self {
            app: None,
            http,
            install_lock: Mutex::new(()),
            model_dir_override: None,
            verified_files: StdMutex::new(HashMap::new()),
        })
    }

    #[cfg(test)]
    pub fn with_model_dir(dir: PathBuf, http: reqwest::Client) -> Arc<Self> {
        Arc::new(Self {
            app: None,
            http,
            install_lock: Mutex::new(()),
            model_dir_override: Some(dir),
            verified_files: StdMutex::new(HashMap::new()),
        })
    }

    pub fn model_dir(&self) -> Result<PathBuf, String> {
        if let Some(dir) = &self.model_dir_override {
            return Ok(dir.clone());
        }
        let app = self
            .app
            .as_ref()
            .ok_or_else(|| "offline models disabled".to_string())?;
        Ok(app
            .path()
            .app_data_dir()
            .map_err(|e| e.to_string())?
            .join("rapidocr-models"))
    }

    pub fn rapidocr_ready(&self, tier: OcrModelTier) -> bool {
        let Ok(dir) = self.model_dir() else {
            return false;
        };
        rapidocr_pack(tier).iter().all(|file| {
            self.validation_state(&dir.join(file.relative_path), file) == ValidationState::Ready
        })
    }

    /// 替换翻译离线包 = RapidOCR 包（ONNX Runtime + 检测/识别模型 + 字典）。
    /// 曾额外含 MI-GAN 修复模型；擦除改为块级盖板后不再需要任何修复模型。
    pub fn replace_translation_status(&self, tier: OcrModelTier) -> ReplaceTranslationPackStatus {
        let Ok(dir) = self.model_dir() else {
            return ReplaceTranslationPackStatus {
                tier: tier.as_str().into(),
                ready: false,
                total_bytes: pack_download_size(&rapidocr_pack(tier)),
                ready_bytes: 0,
                missing_bytes: pack_download_size(&rapidocr_pack(tier)),
                model_dir: None,
                files: Vec::new(),
            };
        };
        self.status_for_files(tier, &dir, &rapidocr_pack(tier))
    }

    pub async fn install_rapidocr(&self, tier: OcrModelTier) -> OfflineModelInstallResult {
        self.install(
            OfflineModelPack::Rapidocr,
            tier,
            rapidocr_pack(tier),
            "RapidOCR 包下载完成",
        )
        .await
    }

    pub async fn install_replace_translation(
        &self,
        tier: OcrModelTier,
    ) -> OfflineModelInstallResult {
        self.install(
            OfflineModelPack::ReplaceTranslation,
            tier,
            rapidocr_pack(tier),
            "替换翻译离线包下载完成",
        )
        .await
    }

    fn status_for_files(
        &self,
        tier: OcrModelTier,
        dir: &Path,
        files: &[ModelFile],
    ) -> ReplaceTranslationPackStatus {
        let total_bytes = pack_download_size(files);
        let mut source_ready = HashMap::<&str, bool>::new();
        let mut source_size = HashMap::<&str, u64>::new();
        let mut statuses = Vec::with_capacity(files.len());

        for file in files {
            let final_path = dir.join(file.relative_path);
            let state = self.validation_state(&final_path, file);
            source_ready
                .entry(file.source.key())
                .and_modify(|ready| *ready &= state == ValidationState::Ready)
                .or_insert(state == ValidationState::Ready);
            source_size.insert(file.source.key(), file.source.size());
            statuses.push(OfflineModelFileStatus {
                component_id: file.component_id.into(),
                file_name: file.relative_path.into(),
                // Actual bytes on disk (0 when missing), NOT the manifest's
                // expected size — the settings panel shows real install state.
                installed_bytes: std::fs::metadata(&final_path)
                    .map(|metadata| metadata.len())
                    .unwrap_or(0),
                download_bytes: file.source.size(),
                ready: state == ValidationState::Ready,
                state: match state {
                    ValidationState::Ready => OfflineModelFileState::Ready,
                    ValidationState::Missing => OfflineModelFileState::Missing,
                    ValidationState::Invalid => OfflineModelFileState::Invalid,
                },
                error: (state == ValidationState::Invalid)
                    .then(|| "size_or_sha256_mismatch".to_string()),
            });
        }

        let ready_bytes = source_ready
            .iter()
            .filter_map(|(key, ready)| ready.then_some(source_size[key]))
            .sum();
        ReplaceTranslationPackStatus {
            tier: tier.as_str().into(),
            ready: statuses.iter().all(|file| file.ready),
            total_bytes,
            ready_bytes,
            missing_bytes: total_bytes.saturating_sub(ready_bytes),
            model_dir: Some(dir.to_string_lossy().into_owned()),
            files: statuses,
        }
    }

    async fn install(
        &self,
        pack: OfflineModelPack,
        tier: OcrModelTier,
        files: Vec<ModelFile>,
        success_message: &str,
    ) -> OfflineModelInstallResult {
        let _guard = self.install_lock.lock().await;
        let dir = match self.model_dir() {
            Ok(dir) => dir,
            Err(error) => return install_failure(error),
        };
        if let Err(error) = tokio::fs::create_dir_all(&dir).await {
            return install_failure(format!("mkdir failed: {error}"));
        }

        let total_bytes = pack_download_size(&files);
        let mut completed_sources = HashSet::new();
        for file in &files {
            if self.validation_state(&dir.join(file.relative_path), file) == ValidationState::Ready
            {
                completed_sources.insert(file.source.key());
            }
        }
        // A shared archive only counts as complete when all outputs sourced from it are ready.
        for source in files
            .iter()
            .map(|file| file.source.key())
            .collect::<HashSet<_>>()
        {
            if files.iter().any(|file| {
                file.source.key() == source
                    && self.validation_state(&dir.join(file.relative_path), file)
                        != ValidationState::Ready
            }) {
                completed_sources.remove(source);
            }
        }
        let mut overall_base: u64 = files
            .iter()
            .filter(|file| completed_sources.contains(file.source.key()))
            .map(|file| (file.source.key(), file.source.size()))
            .collect::<HashMap<_, _>>()
            .into_values()
            .sum();

        for file in &files {
            let final_path = dir.join(file.relative_path);
            if self.validation_state(&final_path, file) == ValidationState::Ready {
                continue;
            }
            if let Some(parent) = final_path.parent() {
                if let Err(error) = tokio::fs::create_dir_all(parent).await {
                    return install_failure(format!("mkdir failed: {error}"));
                }
            }

            let source_was_complete = completed_sources.contains(file.source.key());
            let result = match file.source {
                ModelSource::Direct { .. } => {
                    self.download_direct(pack, file, &final_path, overall_base, total_bytes)
                        .await
                }
                ModelSource::Archive { cache_key, .. } => {
                    self.install_archive_file(
                        pack,
                        file,
                        &dir,
                        cache_key,
                        overall_base,
                        total_bytes,
                    )
                    .await
                }
            };

            if let Err(error) = result {
                self.emit_progress(
                    pack,
                    file,
                    0,
                    overall_base,
                    total_bytes,
                    MAX_ATTEMPTS,
                    OfflineModelProgressState::Failed,
                    Some(error.clone()),
                );
                return install_failure(error);
            }

            if !source_was_complete
                && files.iter().all(|candidate| {
                    candidate.source.key() != file.source.key()
                        || self.validation_state(&dir.join(candidate.relative_path), candidate)
                            == ValidationState::Ready
                })
            {
                completed_sources.insert(file.source.key());
                overall_base = overall_base.saturating_add(file.source.size());
            }
        }

        self.cleanup_archive_cache(&dir, &files).await;
        let status = self.status_for_files(tier, &dir, &files);
        if !status.ready {
            return install_failure("offline model verification failed".into());
        }
        OfflineModelInstallResult {
            success: true,
            message: success_message.into(),
        }
    }

    async fn download_direct(
        &self,
        pack: OfflineModelPack,
        file: &ModelFile,
        final_path: &Path,
        overall_base: u64,
        overall_total: u64,
    ) -> Result<(), String> {
        let part_path = part_path(final_path);
        self.download_source(
            pack,
            file,
            file.source,
            &part_path,
            overall_base,
            overall_total,
        )
        .await?;
        self.emit_progress(
            pack,
            file,
            file.source.size(),
            overall_base.saturating_add(file.source.size()),
            overall_total,
            1,
            OfflineModelProgressState::Verifying,
            None,
        );
        verify_path_async(&part_path, file.installed_size, file.installed_sha256).await?;
        replace_file(&part_path, final_path).await?;
        self.remember_verified(final_path, file.installed_size);
        self.emit_progress(
            pack,
            file,
            file.source.size(),
            overall_base.saturating_add(file.source.size()),
            overall_total,
            1,
            OfflineModelProgressState::Completed,
            None,
        );
        Ok(())
    }

    async fn install_archive_file(
        &self,
        pack: OfflineModelPack,
        file: &ModelFile,
        dir: &Path,
        cache_key: &str,
        overall_base: u64,
        overall_total: u64,
    ) -> Result<(), String> {
        let download_dir = dir.join(".downloads");
        tokio::fs::create_dir_all(&download_dir)
            .await
            .map_err(|e| format!("mkdir download cache: {e}"))?;
        let archive_path = download_dir.join(cache_key);
        if verify_path_async(&archive_path, file.source.size(), file.source.sha256())
            .await
            .is_err()
        {
            let part_path = part_path(&archive_path);
            self.download_source(
                pack,
                file,
                file.source,
                &part_path,
                overall_base,
                overall_total,
            )
            .await?;
            self.emit_progress(
                pack,
                file,
                file.source.size(),
                overall_base.saturating_add(file.source.size()),
                overall_total,
                1,
                OfflineModelProgressState::Verifying,
                None,
            );
            verify_path_async(&part_path, file.source.size(), file.source.sha256()).await?;
            replace_file(&part_path, &archive_path).await?;
        }

        let ModelSource::Archive { entry_suffix, .. } = file.source else {
            return Err("not an archive source".into());
        };
        self.emit_progress(
            pack,
            file,
            file.source.size(),
            overall_base.saturating_add(file.source.size()),
            overall_total,
            1,
            OfflineModelProgressState::Extracting,
            None,
        );
        let output = dir.join(file.relative_path);
        let output_part = part_path(&output);
        let archive = archive_path.clone();
        let suffix = entry_suffix.to_string();
        let output_for_extract = output_part.clone();
        tokio::task::spawn_blocking(move || {
            extract_archive_entry(&archive, &suffix, &output_for_extract)
        })
        .await
        .map_err(|e| format!("extract {} task failed: {e}", file.relative_path))??;
        verify_path_async(&output_part, file.installed_size, file.installed_sha256).await?;
        replace_file(&output_part, &output).await?;
        self.remember_verified(&output, file.installed_size);
        self.emit_progress(
            pack,
            file,
            file.source.size(),
            overall_base.saturating_add(file.source.size()),
            overall_total,
            1,
            OfflineModelProgressState::Completed,
            None,
        );
        Ok(())
    }

    async fn download_source(
        &self,
        pack: OfflineModelPack,
        file: &ModelFile,
        source: ModelSource,
        part_path: &Path,
        overall_base: u64,
        overall_total: u64,
    ) -> Result<(), String> {
        if let Some(parent) = part_path.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(|e| format!("mkdir {}: {e}", parent.display()))?;
        }

        let mut last_error = String::new();
        for attempt in 1..=MAX_ATTEMPTS {
            let mut offset = tokio::fs::metadata(part_path)
                .await
                .map(|metadata| metadata.len())
                .unwrap_or(0);
            if offset > source.size() {
                let _ = tokio::fs::remove_file(part_path).await;
                offset = 0;
            }
            if offset == source.size() {
                if verify_path_async(part_path, source.size(), source.sha256())
                    .await
                    .is_ok()
                {
                    return Ok(());
                }
                let _ = tokio::fs::remove_file(part_path).await;
                offset = 0;
            }

            self.emit_progress(
                pack,
                file,
                offset,
                overall_base.saturating_add(offset),
                overall_total,
                attempt,
                if attempt == 1 {
                    OfflineModelProgressState::Downloading
                } else {
                    OfflineModelProgressState::Retrying
                },
                (attempt > 1).then(|| last_error.clone()),
            );

            match self
                .download_attempt(
                    pack,
                    file,
                    source,
                    part_path,
                    offset,
                    overall_base,
                    overall_total,
                    attempt,
                )
                .await
            {
                Ok(()) => {
                    self.emit_progress(
                        pack,
                        file,
                        source.size(),
                        overall_base.saturating_add(source.size()),
                        overall_total,
                        attempt,
                        OfflineModelProgressState::Verifying,
                        None,
                    );
                    match verify_path_async(part_path, source.size(), source.sha256()).await {
                        Ok(()) => return Ok(()),
                        Err(error) => {
                            last_error = error;
                            let _ = tokio::fs::remove_file(part_path).await;
                        }
                    }
                }
                Err(error) => last_error = error,
            }
            if attempt < MAX_ATTEMPTS {
                tokio::time::sleep(retry_delay(attempt)).await;
            }
        }
        Err(format!(
            "download {} failed after {MAX_ATTEMPTS} attempts: {last_error}",
            file.relative_path
        ))
    }

    #[allow(clippy::too_many_arguments)]
    async fn download_attempt(
        &self,
        pack: OfflineModelPack,
        file: &ModelFile,
        source: ModelSource,
        part_path: &Path,
        requested_offset: u64,
        overall_base: u64,
        overall_total: u64,
        attempt: u8,
    ) -> Result<(), String> {
        let mut request = self
            .http
            .get(source.url())
            .header(USER_AGENT, DOWNLOAD_USER_AGENT)
            .header(ACCEPT_ENCODING, "identity")
            .timeout(DOWNLOAD_TIMEOUT);
        if requested_offset > 0 {
            request = request.header(RANGE, format!("bytes={requested_offset}-"));
        }
        let mut response = request
            .send()
            .await
            .map_err(|e| format!("connect {}: {e}", file.relative_path))?;

        let status = response.status();
        if status.as_u16() == 416 && requested_offset == source.size() {
            return Ok(());
        }
        if !status.is_success() {
            return Err(format!("HTTP {}: {status}", file.relative_path));
        }

        let resume_accepted = requested_offset > 0
            && status == reqwest::StatusCode::PARTIAL_CONTENT
            && content_range_starts_at(response.headers().get(CONTENT_RANGE), requested_offset);
        let start = if resume_accepted { requested_offset } else { 0 };
        let mut output = tokio::fs::OpenOptions::new()
            .create(true)
            .write(true)
            .append(resume_accepted)
            .truncate(!resume_accepted)
            .open(part_path)
            .await
            .map_err(|e| format!("open {}: {e}", part_path.display()))?;
        let mut downloaded = start;
        // Throttle per-chunk progress emits: a 74MB archive yields thousands of
        // reqwest chunks; emit at most every PROGRESS_EMIT_INTERVAL or every
        // PROGRESS_EMIT_BYTES, plus one final emit for the last chunk so the UI
        // always sees the terminal byte count.
        let mut last_emit_at = std::time::Instant::now();
        let mut last_emit_bytes = downloaded;
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|e| format!("read {}: {e}", file.relative_path))?
        {
            output
                .write_all(&chunk)
                .await
                .map_err(|e| format!("write {}: {e}", part_path.display()))?;
            downloaded = downloaded.saturating_add(chunk.len() as u64);
            if downloaded > source.size() {
                return Err(format!(
                    "{} exceeded expected size {}",
                    file.relative_path,
                    source.size()
                ));
            }
            let is_final_chunk = downloaded == source.size();
            if is_final_chunk
                || last_emit_at.elapsed() >= PROGRESS_EMIT_INTERVAL
                || downloaded.saturating_sub(last_emit_bytes) >= PROGRESS_EMIT_BYTES
            {
                self.emit_progress(
                    pack,
                    file,
                    downloaded,
                    overall_base.saturating_add(downloaded),
                    overall_total,
                    attempt,
                    OfflineModelProgressState::Downloading,
                    None,
                );
                last_emit_at = std::time::Instant::now();
                last_emit_bytes = downloaded;
            }
        }
        output
            .flush()
            .await
            .map_err(|e| format!("flush {}: {e}", part_path.display()))?;
        if downloaded != source.size() {
            return Err(format!(
                "{} size mismatch: expected {}, got {downloaded}",
                file.relative_path,
                source.size()
            ));
        }
        Ok(())
    }

    async fn cleanup_archive_cache(&self, dir: &Path, files: &[ModelFile]) {
        let mut cache_keys = HashSet::new();
        for file in files {
            if let ModelSource::Archive { cache_key, .. } = file.source {
                cache_keys.insert(cache_key);
            }
        }
        for key in cache_keys {
            let _ = tokio::fs::remove_file(dir.join(".downloads").join(key)).await;
        }
    }

    fn validation_state(&self, path: &Path, file: &ModelFile) -> ValidationState {
        let Ok(metadata) = path.metadata() else {
            return ValidationState::Missing;
        };
        if !metadata.is_file() || metadata.len() != file.installed_size {
            return ValidationState::Invalid;
        }
        let modified = metadata.modified().ok();
        if self
            .verified_files
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(path)
            .is_some_and(|cached| *cached == (metadata.len(), modified))
        {
            return ValidationState::Ready;
        }
        match sha256_file(path) {
            Ok(hash) if hash == file.installed_sha256 => {
                self.verified_files
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .insert(path.to_path_buf(), (metadata.len(), modified));
                ValidationState::Ready
            }
            _ => ValidationState::Invalid,
        }
    }

    fn remember_verified(&self, path: &Path, expected_size: u64) {
        let modified = path
            .metadata()
            .ok()
            .and_then(|metadata| metadata.modified().ok());
        self.verified_files
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(path.to_path_buf(), (expected_size, modified));
    }

    #[allow(clippy::too_many_arguments)]
    fn emit_progress(
        &self,
        pack: OfflineModelPack,
        file: &ModelFile,
        downloaded_bytes: u64,
        overall_downloaded_bytes: u64,
        overall_total_bytes: u64,
        attempt: u8,
        state: OfflineModelProgressState,
        error: Option<String>,
    ) {
        let Some(app) = &self.app else {
            return;
        };
        let _ = app.emit(
            "replace-translation-pack-progress",
            OfflineModelProgress {
                pack,
                component_id: file.component_id.into(),
                file_name: file.relative_path.into(),
                downloaded_bytes,
                file_total_bytes: file.source.size(),
                overall_downloaded_bytes: overall_downloaded_bytes.min(overall_total_bytes),
                overall_total_bytes,
                attempt,
                state,
                error,
            },
        );
    }
}

fn pack_download_size(files: &[ModelFile]) -> u64 {
    files
        .iter()
        .map(|file| (file.source.key(), file.source.size()))
        .collect::<HashMap<_, _>>()
        .into_values()
        .sum()
}

fn content_range_starts_at(value: Option<&reqwest::header::HeaderValue>, expected: u64) -> bool {
    value
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("bytes "))
        .and_then(|value| value.split_once('-'))
        .and_then(|(start, _)| start.parse::<u64>().ok())
        == Some(expected)
}

fn part_path(path: &Path) -> PathBuf {
    let mut name = path.as_os_str().to_os_string();
    name.push(".part");
    PathBuf::from(name)
}

async fn replace_file(source: &Path, destination: &Path) -> Result<(), String> {
    #[cfg(not(target_os = "windows"))]
    {
        return tokio::fs::rename(source, destination)
            .await
            .map_err(|e| format!("rename {}: {e}", destination.display()));
    }
    #[cfg(target_os = "windows")]
    {
        if destination.exists() {
            if destination.is_file() {
                tokio::fs::remove_file(destination)
                    .await
                    .map_err(|e| format!("remove {}: {e}", destination.display()))?;
            } else {
                return Err(format!(
                    "{} exists but is not a file",
                    destination.display()
                ));
            }
        }
        tokio::fs::rename(source, destination)
            .await
            .map_err(|e| format!("rename {}: {e}", destination.display()))
    }
}

async fn verify_path_async(path: &Path, size: u64, sha256: &str) -> Result<(), String> {
    let path = path.to_path_buf();
    let expected = sha256.to_string();
    tokio::task::spawn_blocking(move || verify_path(&path, size, &expected))
        .await
        .map_err(|e| format!("verify task failed: {e}"))?
}

fn verify_path(path: &Path, size: u64, sha256: &str) -> Result<(), String> {
    let metadata = path
        .metadata()
        .map_err(|e| format!("stat {}: {e}", path.display()))?;
    if !metadata.is_file() || metadata.len() != size {
        return Err(format!(
            "{} size mismatch: expected {size}, got {}",
            path.display(),
            metadata.len()
        ));
    }
    let actual = sha256_file(path)?;
    if actual != sha256 {
        return Err(format!(
            "{} sha256 mismatch: expected {sha256}, got {actual}",
            path.display()
        ));
    }
    Ok(())
}

fn sha256_file(path: &Path) -> Result<String, String> {
    let mut file =
        std::fs::File::open(path).map_err(|e| format!("open {}: {e}", path.display()))?;
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 128 * 1024];
    loop {
        let read = file
            .read(&mut buffer)
            .map_err(|e| format!("read {}: {e}", path.display()))?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

fn install_failure(message: String) -> OfflineModelInstallResult {
    OfflineModelInstallResult {
        success: false,
        message,
    }
}

fn retry_delay(attempt: u8) -> Duration {
    #[cfg(test)]
    {
        let _ = attempt;
        Duration::from_millis(1)
    }
    #[cfg(not(test))]
    {
        Duration::from_millis(400 * 2u64.pow((attempt - 1) as u32))
    }
}

#[cfg(target_os = "macos")]
fn extract_archive_entry(
    archive_path: &Path,
    entry_suffix: &str,
    output: &Path,
) -> Result<(), String> {
    let archive_file = std::fs::File::open(archive_path).map_err(|e| e.to_string())?;
    let reader = GzDecoder::new(archive_file);
    let mut archive = tar::Archive::new(reader);
    for entry in archive.entries().map_err(|e| e.to_string())? {
        let mut entry = entry.map_err(|e| e.to_string())?;
        if entry
            .path()
            .map_err(|e| e.to_string())?
            .to_string_lossy()
            .ends_with(entry_suffix)
        {
            let mut file = std::fs::File::create(output).map_err(|e| e.to_string())?;
            std::io::copy(&mut entry, &mut file).map_err(|e| e.to_string())?;
            return Ok(());
        }
    }
    Err(format!("{entry_suffix} not found in archive"))
}

#[cfg(target_os = "windows")]
fn extract_archive_entry(
    archive_path: &Path,
    entry_suffix: &str,
    output: &Path,
) -> Result<(), String> {
    let archive_file = std::fs::File::open(archive_path).map_err(|e| e.to_string())?;
    let mut archive = zip::ZipArchive::new(archive_file).map_err(|e| e.to_string())?;
    for index in 0..archive.len() {
        let mut entry = archive.by_index(index).map_err(|e| e.to_string())?;
        if entry.name().ends_with(entry_suffix) {
            let mut file = std::fs::File::create(output).map_err(|e| e.to_string())?;
            std::io::copy(&mut entry, &mut file).map_err(|e| e.to_string())?;
            return Ok(());
        }
    }
    Err(format!("{entry_suffix} not found in archive"))
}

static ORT_INIT: OnceCell<()> = OnceCell::const_new();

pub async fn ensure_ort_init(manager: &OfflineModelManager) -> Result<(), String> {
    let dir = manager.model_dir()?;
    if runtime_files().iter().any(|file| {
        manager.validation_state(&dir.join(file.relative_path), file) != ValidationState::Ready
    }) {
        return Err("onnxruntime_missing".into());
    }
    ORT_INIT
        .get_or_try_init(|| async {
            init_ort_from_model_dir(&dir)?;
            Ok::<_, String>(())
        })
        .await?;
    Ok(())
}

/// Initialize ONNX Runtime from an already verified model directory.
///
/// The GUI uses [`ensure_ort_init`] for its process-wide singleton. The one-shot RapidOCR worker
/// calls this directly because it owns a fresh process and exits immediately after one inference.
pub fn init_ort_from_model_dir(dir: &Path) -> Result<(), String> {
    prepare_onnxruntime_dll_dir(dir)?;
    ort::init_from(dir.join(ONNX_RUNTIME_FILE_NAME))
        .map_err(|e| format!("ort init_from failed: {e}"))?
        .commit();
    Ok(())
}

#[cfg(target_os = "windows")]
fn prepare_onnxruntime_dll_dir(dir: &Path) -> Result<(), String> {
    unsafe {
        for dll in [ONNX_RUNTIME_FILE_NAME, PROVIDERS_SHARED_NAME] {
            let mut path: Vec<u16> = dir.join(dll).as_os_str().encode_wide().collect();
            path.push(0);
            let _ = LoadLibraryExW(
                PCWSTR::from_raw(path.as_ptr()),
                None,
                LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR | LOAD_LIBRARY_SEARCH_SYSTEM32,
            )
            .map_err(|e| format!("preload {dll} failed: {e}"))?;
        }
    }
    Ok(())
}

#[cfg(not(target_os = "windows"))]
fn prepare_onnxruntime_dll_dir(_dir: &Path) -> Result<(), String> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write as _;
    use std::net::TcpListener;
    use std::thread::JoinHandle;

    struct TestResponse {
        status: u16,
        body: Vec<u8>,
        declared_length: usize,
        content_range: Option<String>,
        expected_request: Option<String>,
    }

    fn spawn_server(responses: Vec<TestResponse>) -> (String, JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind test server");
        let address = listener.local_addr().expect("test server address");
        let handle = std::thread::spawn(move || {
            for response in responses {
                let (mut stream, _) = listener.accept().expect("accept request");
                let mut request = vec![0u8; 8192];
                let read = stream.read(&mut request).expect("read request");
                let request = String::from_utf8_lossy(&request[..read]);
                if let Some(expected) = response.expected_request {
                    assert!(
                        request
                            .to_ascii_lowercase()
                            .contains(&expected.to_ascii_lowercase()),
                        "request did not contain {expected:?}: {request}"
                    );
                }
                let reason = if response.status == 206 {
                    "Partial Content"
                } else {
                    "OK"
                };
                let mut headers = format!(
                    "HTTP/1.1 {} {}\r\nContent-Length: {}\r\nConnection: close\r\n",
                    response.status, reason, response.declared_length
                );
                if let Some(content_range) = response.content_range {
                    headers.push_str(&format!("Content-Range: {content_range}\r\n"));
                }
                headers.push_str("\r\n");
                stream.write_all(headers.as_bytes()).expect("write headers");
                stream.write_all(&response.body).expect("write body");
                let _ = stream.flush();
            }
        });
        (format!("http://{address}/model.bin"), handle)
    }

    fn direct_test_file(url: String, data: &[u8]) -> ModelFile {
        let url = Box::leak(url.into_boxed_str());
        let sha256 = Box::leak(format!("{:x}", Sha256::digest(data)).into_boxed_str());
        ModelFile {
            component_id: "test",
            relative_path: "model.bin",
            installed_size: data.len() as u64,
            installed_sha256: sha256,
            source: ModelSource::Direct {
                url,
                size: data.len() as u64,
                sha256,
            },
        }
    }

    fn test_dir() -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("kivio-offline-models-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).expect("create temp dir");
        dir
    }

    #[test]
    fn pack_size_deduplicates_shared_archives() {
        let files = rapidocr_pack(OcrModelTier::High);
        let expected = RUNTIME_SOURCE.size()
            + HIGH_DET.source.size()
            + HIGH_REC.source.size()
            + HIGH_KEYS.source.size();
        assert_eq!(pack_download_size(&files), expected);
    }

    #[test]
    fn content_range_requires_exact_resume_offset() {
        let exact = reqwest::header::HeaderValue::from_static("bytes 123-999/1000");
        let wrong = reqwest::header::HeaderValue::from_static("bytes 0-999/1000");
        assert!(content_range_starts_at(Some(&exact), 123));
        assert!(!content_range_starts_at(Some(&wrong), 123));
        assert!(!content_range_starts_at(None, 123));
    }

    #[tokio::test]
    async fn resumes_partial_download_when_server_accepts_range() {
        let data = b"abcdefghij";
        let (url, server) = spawn_server(vec![TestResponse {
            status: 206,
            body: data[4..].to_vec(),
            declared_length: data.len() - 4,
            content_range: Some("bytes 4-9/10".into()),
            expected_request: Some("range: bytes=4-".into()),
        }]);
        let dir = test_dir();
        let part = dir.join("model.bin.part");
        std::fs::write(&part, &data[..4]).expect("seed partial file");
        let manager = OfflineModelManager::with_model_dir(dir.clone(), reqwest::Client::new());
        let file = direct_test_file(url, data);

        manager
            .download_source(
                OfflineModelPack::ReplaceTranslation,
                &file,
                file.source,
                &part,
                0,
                data.len() as u64,
            )
            .await
            .expect("resume download");

        assert_eq!(std::fs::read(&part).expect("read result"), data);
        server.join().expect("server thread");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn restarts_from_zero_when_server_ignores_range() {
        let data = b"abcdefghij";
        let (url, server) = spawn_server(vec![TestResponse {
            status: 200,
            body: data.to_vec(),
            declared_length: data.len(),
            content_range: None,
            expected_request: Some("range: bytes=4-".into()),
        }]);
        let dir = test_dir();
        let part = dir.join("model.bin.part");
        std::fs::write(&part, &data[..4]).expect("seed partial file");
        let manager = OfflineModelManager::with_model_dir(dir.clone(), reqwest::Client::new());
        let file = direct_test_file(url, data);

        manager
            .download_source(
                OfflineModelPack::ReplaceTranslation,
                &file,
                file.source,
                &part,
                0,
                data.len() as u64,
            )
            .await
            .expect("restart download");

        assert_eq!(std::fs::read(&part).expect("read result"), data);
        server.join().expect("server thread");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn retries_checksum_mismatch_and_keeps_only_verified_bytes() {
        let data = b"abcdefghij";
        let wrong = b"0123456789";
        let (url, server) = spawn_server(vec![
            TestResponse {
                status: 200,
                body: wrong.to_vec(),
                declared_length: wrong.len(),
                content_range: None,
                expected_request: None,
            },
            TestResponse {
                status: 200,
                body: data.to_vec(),
                declared_length: data.len(),
                content_range: None,
                expected_request: None,
            },
        ]);
        let dir = test_dir();
        let part = dir.join("model.bin.part");
        let manager = OfflineModelManager::with_model_dir(dir.clone(), reqwest::Client::new());
        let file = direct_test_file(url, data);

        manager
            .download_source(
                OfflineModelPack::ReplaceTranslation,
                &file,
                file.source,
                &part,
                0,
                data.len() as u64,
            )
            .await
            .expect("retry checksum mismatch");

        assert_eq!(std::fs::read(&part).expect("read result"), data);
        server.join().expect("server thread");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn resumes_after_interrupted_response_body() {
        let data = b"abcdefghij";
        let (url, server) = spawn_server(vec![
            TestResponse {
                status: 200,
                body: data[..4].to_vec(),
                declared_length: data.len(),
                content_range: None,
                expected_request: None,
            },
            TestResponse {
                status: 206,
                body: data[4..].to_vec(),
                declared_length: data.len() - 4,
                content_range: Some("bytes 4-9/10".into()),
                expected_request: Some("range: bytes=4-".into()),
            },
        ]);
        let dir = test_dir();
        let part = dir.join("model.bin.part");
        let manager = OfflineModelManager::with_model_dir(dir.clone(), reqwest::Client::new());
        let file = direct_test_file(url, data);

        manager
            .download_source(
                OfflineModelPack::ReplaceTranslation,
                &file,
                file.source,
                &part,
                0,
                data.len() as u64,
            )
            .await
            .expect("resume interrupted body");

        assert_eq!(std::fs::read(&part).expect("read result"), data);
        server.join().expect("server thread");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn skips_network_when_complete_part_is_already_verified() {
        let data = b"abcdefghij";
        let dir = test_dir();
        let part = dir.join("model.bin.part");
        std::fs::write(&part, data).expect("seed completed part");
        let manager = OfflineModelManager::with_model_dir(dir.clone(), reqwest::Client::new());
        let file = direct_test_file("http://127.0.0.1:1/unreachable".into(), data);

        manager
            .download_source(
                OfflineModelPack::ReplaceTranslation,
                &file,
                file.source,
                &part,
                0,
                data.len() as u64,
            )
            .await
            .expect("skip completed part");

        assert_eq!(std::fs::read(&part).expect("read result"), data);
        let _ = std::fs::remove_dir_all(dir);
    }
}

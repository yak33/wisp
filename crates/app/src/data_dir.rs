//! 发行模式与用户数据目录。
//!
//! 同一份 `wisp.exe` 通过同目录的 `portable.flag` 区分发行形态：
//! - 安装版继续使用 `%LOCALAPPDATA%\Wisp`；
//! - 便携版使用程序同目录的 `data`，可随整个目录迁移。

use std::{
    fs,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{Context as _, Result, anyhow};
use gpui::Global;

const PORTABLE_MARKER: &str = "portable.flag";
const DATA_DIRECTORY: &str = "data";
const MANAGED_FILES: [&str; 3] = ["wisp.db", "wisp.db-wal", "wisp.cfg"];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DistributionMode {
    Installed,
    Portable,
}

impl DistributionMode {
    pub(crate) const fn name(self) -> &'static str {
        match self {
            Self::Installed => "安装版",
            Self::Portable => "便携版",
        }
    }
}

/// 当前实例的数据位置。它在应用启动前确定，运行期间保持不变。
#[derive(Debug, Clone)]
pub(crate) struct DataDirectory {
    mode: DistributionMode,
    root: PathBuf,
    installed_root: PathBuf,
}

impl Global for DataDirectory {}

impl DataDirectory {
    pub(crate) fn resolve() -> Result<Self> {
        let executable = std::env::current_exe().context("无法确定 Wisp 程序路径")?;
        let local_app_data = std::env::var_os("LOCALAPPDATA")
            .map(PathBuf::from)
            .ok_or_else(|| anyhow!("Windows 未提供 LOCALAPPDATA 目录"))?;
        Self::from_paths(&executable, &local_app_data)
    }

    fn from_paths(executable: &Path, local_app_data: &Path) -> Result<Self> {
        let executable_dir = executable
            .parent()
            .filter(|path| !path.as_os_str().is_empty())
            .ok_or_else(|| anyhow!("Wisp 程序路径缺少父目录"))?;
        let installed_root = local_app_data.join("Wisp");
        let portable = executable_dir.join(PORTABLE_MARKER).is_file();

        Ok(Self {
            mode: if portable {
                DistributionMode::Portable
            } else {
                DistributionMode::Installed
            },
            root: if portable {
                executable_dir.join(DATA_DIRECTORY)
            } else {
                installed_root.clone()
            },
            installed_root,
        })
    }

    pub(crate) const fn mode(&self) -> DistributionMode {
        self.mode
    }

    pub(crate) fn database_path(&self) -> PathBuf {
        self.root.join("wisp.db")
    }

    pub(crate) fn should_offer_installed_data_import(&self) -> bool {
        self.mode == DistributionMode::Portable
            && !self.root.exists()
            && contains_wisp_data(&self.installed_root)
    }

    /// 准备数据目录。导入只发生在便携版首次启动且目标目录尚不存在时；
    /// 先复制到同级临时目录，全部成功后原子改名，避免留下半套数据。
    pub(crate) fn prepare(&self, import_installed_data: bool) -> Result<()> {
        if import_installed_data && self.should_offer_installed_data_import() {
            return import_data_atomically(&self.installed_root, &self.root);
        }

        fs::create_dir_all(&self.root)
            .with_context(|| format!("创建 Wisp 数据目录失败: {}", self.root.display()))
    }
}

fn contains_wisp_data(root: &Path) -> bool {
    MANAGED_FILES.iter().any(|name| root.join(name).is_file())
        || root.join("images").read_dir().is_ok_and(|entries| {
            entries
                .filter_map(|entry| entry.ok())
                .any(|entry| entry.path().is_file())
        })
}

fn import_data_atomically(source: &Path, target: &Path) -> Result<()> {
    let parent = target
        .parent()
        .ok_or_else(|| anyhow!("便携数据目录缺少父目录"))?;
    fs::create_dir_all(parent)
        .with_context(|| format!("创建便携数据父目录失败: {}", parent.display()))?;

    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let staging = parent.join(format!(
        ".{DATA_DIRECTORY}.importing-{}-{nonce}",
        std::process::id()
    ));

    let result = (|| {
        fs::create_dir(&staging)
            .with_context(|| format!("创建便携数据临时目录失败: {}", staging.display()))?;

        for name in MANAGED_FILES {
            copy_if_file(&source.join(name), &staging.join(name))?;
        }
        copy_managed_images(&source.join("images"), &staging.join("images"))?;

        fs::rename(&staging, target).with_context(|| {
            format!(
                "提交便携数据导入失败: {} -> {}",
                staging.display(),
                target.display()
            )
        })
    })();

    if result.is_err() {
        _ = fs::remove_dir_all(&staging);
    }
    result
}

fn copy_if_file(source: &Path, target: &Path) -> Result<()> {
    if !source.is_file() {
        return Ok(());
    }
    fs::copy(source, target).with_context(|| format!("导入数据文件失败: {}", source.display()))?;
    Ok(())
}

fn copy_managed_images(source: &Path, target: &Path) -> Result<()> {
    let Ok(entries) = fs::read_dir(source) else {
        return Ok(());
    };

    for entry in entries {
        let entry = entry.with_context(|| format!("读取图像数据目录失败: {}", source.display()))?;
        let path = entry.path();
        let is_png = path
            .extension()
            .and_then(|extension| extension.to_str())
            .is_some_and(|extension| extension.eq_ignore_ascii_case("png"));
        if !path.is_file() || !is_png {
            continue;
        }

        fs::create_dir_all(target)
            .with_context(|| format!("创建便携图像目录失败: {}", target.display()))?;
        fs::copy(&path, target.join(entry.file_name()))
            .with_context(|| format!("导入剪贴板图像失败: {}", path.display()))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TestDirectory(PathBuf);

    impl TestDirectory {
        fn new(name: &str) -> Self {
            let nonce = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let path = std::env::temp_dir().join(format!(
                "wisp-data-dir-{name}-{}-{nonce}",
                std::process::id()
            ));
            fs::create_dir_all(&path).unwrap();
            Self(path)
        }
    }

    impl Drop for TestDirectory {
        fn drop(&mut self) {
            _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn marker_selects_portable_data_beside_the_executable() {
        let sandbox = TestDirectory::new("portable");
        let executable_dir = sandbox.0.join("bin");
        let local_app_data = sandbox.0.join("local");
        fs::create_dir_all(&executable_dir).unwrap();
        fs::write(executable_dir.join(PORTABLE_MARKER), "portable\n").unwrap();

        let data =
            DataDirectory::from_paths(&executable_dir.join("wisp.exe"), &local_app_data).unwrap();

        assert_eq!(data.mode(), DistributionMode::Portable);
        assert_eq!(
            data.database_path(),
            executable_dir.join("data").join("wisp.db")
        );
    }

    #[test]
    fn missing_marker_keeps_installed_data_in_local_app_data() {
        let sandbox = TestDirectory::new("installed");
        let executable_dir = sandbox.0.join("bin");
        let local_app_data = sandbox.0.join("local");
        fs::create_dir_all(&executable_dir).unwrap();

        let data =
            DataDirectory::from_paths(&executable_dir.join("wisp.exe"), &local_app_data).unwrap();

        assert_eq!(data.mode(), DistributionMode::Installed);
        assert_eq!(
            data.database_path(),
            local_app_data.join("Wisp").join("wisp.db")
        );
    }

    #[test]
    fn portable_import_copies_only_managed_data_and_png_images() {
        let sandbox = TestDirectory::new("import");
        let executable_dir = sandbox.0.join("bin");
        let local_app_data = sandbox.0.join("local");
        let installed = local_app_data.join("Wisp");
        fs::create_dir_all(executable_dir.as_path()).unwrap();
        fs::create_dir_all(installed.join("images")).unwrap();
        fs::write(executable_dir.join(PORTABLE_MARKER), "portable\n").unwrap();
        fs::write(installed.join("wisp.db"), b"database").unwrap();
        fs::write(installed.join("wisp.cfg"), b"theme=dark\n").unwrap();
        fs::write(installed.join("images").join("clip.png"), b"png").unwrap();
        fs::write(installed.join("images").join("ignore.txt"), b"ignore").unwrap();

        let data =
            DataDirectory::from_paths(&executable_dir.join("wisp.exe"), &local_app_data).unwrap();
        assert!(data.should_offer_installed_data_import());
        data.prepare(true).unwrap();

        let portable = executable_dir.join("data");
        assert_eq!(fs::read(portable.join("wisp.db")).unwrap(), b"database");
        assert_eq!(
            fs::read_to_string(portable.join("wisp.cfg")).unwrap(),
            "theme=dark\n"
        );
        assert!(portable.join("images").join("clip.png").is_file());
        assert!(!portable.join("images").join("ignore.txt").exists());
    }
}

//! The `dotnet11` verb: Microsoft .NET Framework 1.1 in a 64-bit prefix (winetricks' installs
//! nothing there: its package refuses 64-bit Windows). Need for Speed Underground's and
//! Carbon's cabinet frontends and data engine run on it.
//!
//! 1. `dotnetfx.exe` from winetricks' cache (`dotnet11/`), else downloaded there (winetricks'
//!    archived Microsoft URL), its SHA-256 checked;
//! 2. extracted by itself (`/q /c /t:`, an IExpress package): `netfx.msi`;
//! 3. the MSI's only launch condition, the string `NOT (VersionNT64)`, made
//!    `NOT (VersionNT65)` (same length), then `msiexec /i netfx.msi /qn`;
//! 4. the Global Assembly Cache, which Wine's msi leaves empty, filled from the framework's
//!    directory by a small program built with the framework's own `csc.exe`
//!    (`System.EnterpriseServices.Internal.Publish.GacInstall`, its System.EnterpriseServices
//!    copied next to it).
//!
//! The framework's own runtime DLLs are native for the games (`trick_dlls`): mscoree, mscorwks,
//! fusion. A prefix that has it already (unpacked from an archive) is left as it is.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};

use super::Wine;

const URL: &str = "https://web.archive.org/web/20210505032023/http://download.microsoft.com/download/a/a/c/aac39226-8825-44ce-90e3-bf8203e74006/dotnetfx.exe";
const SHA256: &str = "ba0e58ec93f2ffd54fc7c627eeca9502e11ab3c6fc85dcbeff113bd61d995bce";
const LAUNCH_CONDITION: &[u8] = b"NOT (VersionNT64)";
const LAUNCH_CONDITION_NEW: &[u8] = b"NOT (VersionNT65)";
/// The DLLs native while installing and for the games.
pub const DLLS: [&str; 3] = ["mscoree", "mscorwks", "fusion"];
const FRAMEWORK: &str = r"C:\windows\Microsoft.NET\Framework\v1.1.4322";

/// Installs every strong-named assembly of the directory given into the GAC.
const GAC_INSTALL_CS: &str = r#"using System;
using System.IO;
using System.Reflection;
using System.EnterpriseServices.Internal;

class GacInstall
{
    static int Main(string[] args)
    {
        Publish publish = new Publish();
        int count = 0;
        foreach (string file in Directory.GetFiles(args[0], "*.dll"))
        {
            try
            {
                AssemblyName name = AssemblyName.GetAssemblyName(file);
                if (name.GetPublicKeyToken() == null || name.GetPublicKeyToken().Length == 0)
                    continue;
                publish.GacInstall(file);
                count++;
            }
            catch (Exception)
            {
            }
        }
        Console.WriteLine(count + " assemblies installed");
        return count > 0 ? 0 : 1;
    }
}
"#;

impl Wine {
    /// Whether the framework is installed with its assembly cache.
    fn dotnet11_installed(&self) -> bool {
        let windows = self.prefix.join("drive_c/windows");
        windows.join("Microsoft.NET/Framework/v1.1.4322/mscorwks.dll").is_file()
            && windows.join("assembly/GAC/System").is_dir()
    }

    /// A wine command with the framework's DLLs native.
    fn dotnet11_command(&self, program: &str) -> Command {
        let mut cmd = self.command(program);
        let mut overrides = self.overrides.clone();
        overrides.push(format!("{}=n", DLLS.join(",")));
        cmd.env("WINEDLLOVERRIDES", overrides.join(";"));
        cmd
    }

    pub(super) fn install_dotnet11(&self) -> Result<()> {
        if self.dotnet11_installed() {
            eprintln!("wine: .NET 1.1 present in the prefix");
            return Ok(());
        }
        let installer = installer()?;
        let work = self.prefix.join("drive_c/windows/temp/wal-dotnet11");
        if work.exists() {
            fs::remove_dir_all(&work)?;
        }
        fs::create_dir_all(&work)?;
        let result = self.install_dotnet11_in(&installer, &work);
        let _ = fs::remove_dir_all(&work);
        result
    }

    fn install_dotnet11_in(&self, installer: &Path, work: &Path) -> Result<()> {
        eprintln!("wine: .NET 1.1: extracting {}", installer.display());
        let status = self
            .command(installer)
            .args(["/q", "/c", &format!("/t:{}", self.windows_path(work))])
            .status()
            .context("running dotnetfx.exe")?;
        self.wineserver("-w")?;
        let msi = work.join("netfx.msi");
        if !status.success() || !msi.is_file() {
            bail!(".NET 1.1: dotnetfx.exe extraction failed ({status})");
        }
        let mut data = fs::read(&msi)?;
        let at = data
            .windows(LAUNCH_CONDITION.len())
            .position(|w| w == LAUNCH_CONDITION)
            .context(".NET 1.1: netfx.msi launch condition not found")?;
        data[at..at + LAUNCH_CONDITION.len()].copy_from_slice(LAUNCH_CONDITION_NEW);
        fs::write(&msi, data)?;

        eprintln!("wine: .NET 1.1: installing netfx.msi");
        let status = self
            .dotnet11_command("msiexec")
            .args(["/i", &self.windows_path(&msi), "/qn"])
            .status()
            .context("running msiexec")?;
        self.wineserver("-w")?;
        if !status.success() {
            bail!(".NET 1.1: msiexec failed ({status})");
        }

        eprintln!("wine: .NET 1.1: filling the assembly cache");
        fs::write(work.join("gacinstall.cs"), GAC_INSTALL_CS)?;
        let status = self
            .dotnet11_command(&format!(r"{FRAMEWORK}\csc.exe"))
            .args(["/nologo", "/r:System.EnterpriseServices.dll", "/out:gacinstall.exe", "gacinstall.cs"])
            .current_dir(work)
            .status()
            .context("running csc.exe")?;
        self.wineserver("-w")?;
        if !status.success() || !work.join("gacinstall.exe").is_file() {
            bail!(".NET 1.1: building the assembly cache installer failed ({status})");
        }
        // its System.EnterpriseServices next to it once built: not in the cache yet (else the
        // runtime's probing ends in a stack overflow)
        let framework = self.prefix.join("drive_c/windows/Microsoft.NET/Framework/v1.1.4322");
        fs::copy(framework.join("System.EnterpriseServices.dll"), work.join("System.EnterpriseServices.dll"))?;
        let status = self
            .dotnet11_command(&self.windows_path(&work.join("gacinstall.exe")))
            .arg(FRAMEWORK)
            .current_dir(work)
            .status()
            .context("running gacinstall.exe")?;
        self.wineserver("-w")?;
        if !status.success() || !self.dotnet11_installed() {
            bail!(".NET 1.1: filling the assembly cache failed ({status})");
        }
        Ok(())
    }
}

/// `dotnetfx.exe`: winetricks' cached copy, else downloaded into its cache.
fn installer() -> Result<PathBuf> {
    let cache = std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache")))
        .context("no cache directory (HOME unset)")?
        .join("winetricks/dotnet11");
    let file = cache.join("dotnetfx.exe");
    if file.is_file() && sha256(&file)? == SHA256 {
        return Ok(file);
    }
    fs::create_dir_all(&cache)?;
    let part = cache.join("dotnetfx.exe.part");
    eprintln!("wine: .NET 1.1: downloading {URL}");
    let curl = Command::new("curl").args(["-fL", "--retry", "3", "-o"]).arg(&part).arg(URL).status();
    let ok = match curl {
        Ok(s) => s.success(),
        Err(_) => Command::new("wget").arg("-O").arg(&part).arg(URL).status().context("running curl / wget")?.success(),
    };
    if !ok {
        let _ = fs::remove_file(&part);
        bail!(".NET 1.1: downloading dotnetfx.exe failed (no internet? copy it to {})", file.display());
    }
    let sum = sha256(&part)?;
    if sum != SHA256 {
        let _ = fs::remove_file(&part);
        bail!(".NET 1.1: dotnetfx.exe checksum {sum}, expected {SHA256}");
    }
    fs::rename(&part, &file)?;
    Ok(file)
}

fn sha256(file: &Path) -> Result<String> {
    let out = Command::new("sha256sum").arg(file).output().context("running sha256sum")?;
    Ok(String::from_utf8_lossy(&out.stdout).split_whitespace().next().unwrap_or_default().to_string())
}

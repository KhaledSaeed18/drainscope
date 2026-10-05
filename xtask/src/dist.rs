//! `dist`: release artifacts in `target/dist/`.
//!
//! - `drainscope-<version>.tar.gz`: the committed tree, the Rust crates vendored for an
//!   offline build (as COPR builds), the bundled GNOME Shell extension and app, and
//!   `LICENSE.dependencies`.
//! - the SRPM built from `packaging/drainscope.spec` (and binary RPMs with `--rpm`).
//! - the extensions.gnome.org upload zip.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail, ensure};

const SPEC: &str = "packaging/drainscope.spec";
const SHIPPED: [&str; 3] = ["drainscope-daemon", "drainscope-sampler", "drainscope-cli"];

fn run(program: &str, args: &[&str], dir: &Path) -> Result<String> {
    let output = Command::new(program)
        .args(args)
        .current_dir(dir)
        .output()
        .with_context(|| format!("running {program}"))?;
    if !output.status.success() {
        bail!(
            "{program} {} failed:\n{}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// The version every shipped crate shares.
fn version(repo: &Path) -> Result<String> {
    let metadata: serde_json::Value = serde_json::from_str(&run(
        "cargo",
        &["metadata", "--format-version", "1", "--no-deps"],
        repo,
    )?)?;
    let versions: BTreeSet<&str> = metadata["packages"]
        .as_array()
        .context("cargo metadata packages")?
        .iter()
        .filter(|p| SHIPPED.contains(&p["name"].as_str().unwrap_or_default()))
        .filter_map(|p| p["version"].as_str())
        .collect();
    match versions.into_iter().collect::<Vec<_>>().as_slice() {
        [version] => Ok((*version).to_owned()),
        other => bail!("shipped crates disagree on their version: {other:?}"),
    }
}

/// `MIT/Apache-2.0` → `Apache-2.0 OR MIT`; `(MIT OR Apache-2.0) AND Unicode-3.0` → its parts.
fn license_terms(expression: &str) -> Vec<String> {
    let plain = expression
        .replace("(*)", "")
        .replace(['(', ')'], "")
        .replace('/', " OR ");
    plain
        .split(" AND ")
        .map(|term| {
            let mut options: Vec<&str> = term.split(" OR ").map(str::trim).collect();
            options.sort_unstable();
            options.dedup();
            if options.len() > 1 {
                format!("({})", options.join(" OR "))
            } else {
                options.concat()
            }
        })
        .filter(|term| !term.is_empty())
        .collect()
}

/// The SPDX expression for the shipped binaries and everything they link, plus the
/// per-crate list for `LICENSE.dependencies`.
fn licenses(repo: &Path) -> Result<(String, String)> {
    let mut args = vec![
        "tree", "-e", "normal", "--prefix", "none", "--format", "{p}\t{l}",
    ];
    for package in SHIPPED {
        args.extend(["-p", package]);
    }
    let tree = run("cargo", &args, repo)?;
    let mut crates = BTreeSet::new();
    let mut terms = BTreeSet::new();
    for line in tree.lines().filter(|l| !l.trim().is_empty()) {
        let (package, license) = line.split_once('\t').unwrap_or((line, ""));
        let package = package.trim_end_matches(" (*)").trim();
        let license = license.replace("(*)", "");
        if license.trim().is_empty() {
            bail!("{package} declares no license");
        }
        terms.extend(license_terms(&license));
        crates.insert(format!("{package}: {}", license.trim()));
    }
    // GPL first (drainscope itself), then the dependencies.
    let gpl = "GPL-3.0-or-later";
    let mut ordered: Vec<String> = terms.iter().filter(|t| *t == gpl).cloned().collect();
    ordered.extend(terms.into_iter().filter(|t| t != gpl));
    let list = crates.into_iter().collect::<Vec<_>>().join("\n") + "\n";
    Ok((ordered.join(" AND "), list))
}

fn spec_license(repo: &Path) -> Result<String> {
    let spec = fs::read_to_string(repo.join(SPEC))?;
    spec.lines()
        .find_map(|l| l.strip_prefix("License:"))
        .map(|l| l.trim().to_owned())
        .context("no License: line in the spec")
}

pub fn run_dist(repo: &Path, binary_rpms: bool) -> Result<()> {
    ensure!(
        run("git", &["status", "--porcelain"], repo)?
            .trim()
            .is_empty(),
        "commit or stash your changes first: the tarball is built from HEAD"
    );
    let version = version(repo)?;
    let (expression, dependency_list) = licenses(repo)?;
    let declared = spec_license(repo)?;
    // SPDX AND lists are unordered: compare the sets of terms.
    let terms = |e: &str| e.split(" AND ").map(str::to_owned).collect::<BTreeSet<_>>();
    ensure!(
        terms(&declared) == terms(&expression),
        "the spec's License tag is out of date.\n  spec:     {declared}\n  computed: {expression}"
    );

    let dist = repo.join("target/dist");
    let tarball = build_tarball(
        repo,
        &dist,
        &format!("drainscope-{version}"),
        &dependency_list,
    )?;
    let topdir = build_rpms(repo, &dist, binary_rpms)?;
    pack_extension(repo, &dist)?;

    println!("tarball   {}", tarball.display());
    for sub in ["SRPMS", "RPMS/x86_64", "RPMS/noarch"] {
        for file in list(&topdir.join(sub))? {
            println!("rpm       {}", file.display());
        }
    }
    for file in list(&dist)?
        .into_iter()
        .filter(|f| f.extension().is_some_and(|e| e == "zip"))
    {
        println!("ego zip   {}", file.display());
    }
    Ok(())
}

/// The committed tree, vendored crates, the built extension and the license list, as
/// `<dist>/<name>.tar.gz`.
fn build_tarball(repo: &Path, dist: &Path, name: &str, dependency_list: &str) -> Result<PathBuf> {
    let staging_parent = dist.join("staging");
    let staging = staging_parent.join(name);
    if staging_parent.exists() {
        fs::remove_dir_all(&staging_parent)?;
    }
    fs::create_dir_all(&staging_parent)?;
    let archive = dist.join("tree.tar");
    let prefix = format!("--prefix={name}/");
    run(
        "git",
        &[
            "archive",
            "--format=tar",
            &prefix,
            "-o",
            &archive.to_string_lossy(),
            "HEAD",
        ],
        repo,
    )?;
    run(
        "tar",
        &[
            "-xf",
            &archive.to_string_lossy(),
            "-C",
            &staging_parent.to_string_lossy(),
        ],
        repo,
    )?;
    fs::remove_file(&archive)?;

    eprintln!("vendoring crates…");
    let vendor = staging.join("vendor");
    run(
        "cargo",
        &[
            "vendor",
            "--locked",
            "--versioned-dirs",
            &vendor.to_string_lossy(),
        ],
        repo,
    )?;
    fs::write(staging.join("LICENSE.dependencies"), dependency_list)?;

    eprintln!("building the GNOME Shell extension and the app…");
    let ui = repo.join("ui");
    run("pnpm", &["install", "--frozen-lockfile"], &ui)?;
    run("pnpm", &["--filter", "@drainscope/extension", "build"], &ui)?;
    copy_dir(
        &ui.join("extension/dist"),
        &staging.join("ui/extension/dist"),
    )?;
    run("pnpm", &["--filter", "@drainscope/app", "build"], &ui)?;
    copy_dir(&ui.join("app/dist"), &staging.join("ui/app/dist"))?;

    let tarball = dist.join(format!("{name}.tar.gz"));
    run(
        "tar",
        &[
            "-czf",
            &tarball.to_string_lossy(),
            "-C",
            &staging_parent.to_string_lossy(),
            name,
        ],
        repo,
    )?;
    fs::remove_dir_all(&staging_parent)?;
    Ok(tarball)
}

/// The SRPM, plus binary RPMs if asked; returns rpmbuild's top directory.
fn build_rpms(repo: &Path, dist: &Path, binary_rpms: bool) -> Result<PathBuf> {
    let topdir = dist.join("rpmbuild");
    let mode = if binary_rpms { "-ba" } else { "-bs" };
    let define_top = format!("_topdir {}", topdir.display());
    let define_sources = format!("_sourcedir {}", dist.display());
    let spec = repo.join(SPEC);
    let spec = spec.to_string_lossy();
    let mut args = vec![mode, "--define", &define_top, "--define", &define_sources];
    if binary_rpms {
        // Local test builds use the toolchain on PATH rather than distro packages.
        args.push("--nodeps");
    }
    args.push(&spec);
    eprintln!("running rpmbuild {mode}…");
    run("rpmbuild", &args, repo)?;
    Ok(topdir)
}

/// The extensions.gnome.org upload zip.
fn pack_extension(repo: &Path, dist: &Path) -> Result<()> {
    let extension = repo.join("ui/extension/dist");
    run(
        "gnome-extensions",
        &[
            "pack",
            &extension.to_string_lossy(),
            "--force",
            "--out-dir",
            &dist.to_string_lossy(),
        ],
        repo,
    )?;
    Ok(())
}

fn list(dir: &Path) -> Result<Vec<PathBuf>> {
    if !dir.exists() {
        return Ok(Vec::new());
    }
    let mut files: Vec<PathBuf> = fs::read_dir(dir)?
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.is_file())
        .collect();
    files.sort();
    Ok(files)
}

fn copy_dir(from: &Path, to: &Path) -> Result<()> {
    fs::create_dir_all(to)?;
    for entry in fs::read_dir(from)? {
        let entry = entry?;
        let target = to.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_dir(&entry.path(), &target)?;
        } else {
            fs::copy(entry.path(), target)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::license_terms;

    #[test]
    fn normalizes_license_expressions() {
        assert_eq!(license_terms("MIT/Apache-2.0"), ["(Apache-2.0 OR MIT)"]);
        assert_eq!(
            license_terms("MIT OR Apache-2.0 (*)"),
            ["(Apache-2.0 OR MIT)"]
        );
        assert_eq!(
            license_terms("(MIT OR Apache-2.0) AND Unicode-3.0"),
            ["(Apache-2.0 OR MIT)", "Unicode-3.0"]
        );
        assert_eq!(
            license_terms("Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT"),
            ["(Apache-2.0 OR Apache-2.0 WITH LLVM-exception OR MIT)"]
        );
        assert_eq!(license_terms("Zlib"), ["Zlib"]);
    }
}

//! Compiles the eBPF programs in `bpf/` with clang into `OUT_DIR`, for `include_bytes!`.

use std::env;
use std::path::PathBuf;
use std::process::Command;

const PROGRAMS: &[&str] = &["wakeups"];

fn main() {
    let out = PathBuf::from(env::var_os("OUT_DIR").unwrap_or_default());
    // __TARGET_ARCH_* selects the register layout bpf_tracing.h uses for this architecture.
    let arch = match env::var("CARGO_CFG_TARGET_ARCH").as_deref() {
        Ok("x86_64") => "x86",
        Ok("aarch64") => "arm64",
        Ok("powerpc64") => "powerpc",
        Ok("s390x") => "s390",
        Ok("riscv64") => "riscv",
        Ok(other) => panic!("no eBPF target architecture for {other}"),
        Err(err) => panic!("CARGO_CFG_TARGET_ARCH: {err}"),
    };
    let clang = env::var("CLANG").unwrap_or_else(|_| "clang".to_owned());
    // Debian and Ubuntu keep <asm/types.h> in a multiarch directory clang doesn't search for
    // the bpf target.
    let multiarch = env::var("CARGO_CFG_TARGET_ARCH")
        .map(|target| format!("/usr/include/{target}-linux-gnu"))
        .ok()
        .filter(|dir| std::path::Path::new(dir).is_dir());
    for program in PROGRAMS {
        let source = format!("bpf/{program}.bpf.c");
        println!("cargo::rerun-if-changed={source}");
        let object = out.join(format!("{program}.bpf.o"));
        let status = Command::new(&clang)
            .args(["-O2", "-g", "-target", "bpf", "-Wall", "-Werror"])
            .arg(format!("-D__TARGET_ARCH_{arch}"))
            .args(multiarch.iter().map(|dir| format!("-I{dir}")))
            .args(["-c", &source, "-o"])
            .arg(&object)
            .status()
            .unwrap_or_else(|err| {
                panic!("running {clang} (install clang to build the probe): {err}")
            });
        assert!(status.success(), "{clang} failed to compile {source}");
    }
    println!("cargo::rerun-if-env-changed=CLANG");
}

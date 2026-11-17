//! The GPU helper binary: its start-up failure without CUDA, and the GPU vectors against the CPU model.

#[cfg(not(feature = "cuda"))]
mod without_cuda {
    use std::fs;
    use std::path::Path;
    use std::process::Command;

    use modfoundry_search::helper::HelperEmbed;

    #[test]
    fn the_helper_says_when_it_was_built_without_cuda() {
        let model = std::env::temp_dir().join(format!("mf-embed-gpu-empty-{}", std::process::id()));
        fs::create_dir_all(&model).unwrap();
        let program = Path::new(env!("CARGO_BIN_EXE_mf-embed-gpu"));
        let err = HelperEmbed::spawn(program, &model, None)
            .err()
            .expect("a build without CUDA cannot start");
        let _ = fs::remove_dir_all(&model);
        assert!(err.contains("not been built with cuda support"), "{err}");
    }

    #[test]
    fn check_says_when_the_helper_has_no_cuda() {
        let out = Command::new(env!("CARGO_BIN_EXE_mf-embed-gpu"))
            .arg("--check")
            .output()
            .unwrap();
        assert_eq!(out.status.code(), Some(1));
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(
            stderr.contains("not been built with cuda support"),
            "{stderr}"
        );

        let usage = Command::new(env!("CARGO_BIN_EXE_mf-embed-gpu"))
            .output()
            .unwrap();
        assert_eq!(usage.status.code(), Some(2));
        let stderr = String::from_utf8_lossy(&usage.stderr);
        assert!(stderr.contains("| --check"), "{stderr}");
    }
}

#[test]
#[ignore = "needs a CUDA build and MF_SEARCH_MODEL"]
fn gpu_vectors_match_the_cpu_model() {
    use modfoundry_search::embed::{BgeSmall, Embed};
    use modfoundry_search::helper::HelperEmbed;
    use std::path::{Path, PathBuf};

    let Some(model) = std::env::var_os("MF_SEARCH_MODEL").map(PathBuf::from) else {
        return;
    };
    let texts = [
        "how do I hide a toast?",
        "class ToastManager { void Hide(); }",
        "apple pie recipe",
    ];
    let program = Path::new(env!("CARGO_BIN_EXE_mf-embed-gpu"));
    let check = std::process::Command::new(program)
        .arg("--check")
        .output()
        .unwrap();
    assert_eq!(
        check.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&check.stderr)
    );
    let gpu = HelperEmbed::spawn(program, &model, None).expect("the GPU helper starts");
    let cpu = BgeSmall::load(&model).expect("the CPU model loads");
    let gpu_vectors = gpu.embed(&texts).expect("the GPU helper embeds");
    let cpu_vectors = cpu.embed(&texts).expect("the CPU model embeds");
    for (gpu, cpu) in gpu_vectors.iter().zip(&cpu_vectors) {
        let dot: f32 = gpu.iter().zip(cpu).map(|(a, b)| a * b).sum();
        assert!(dot > 0.999, "{dot}");
    }
}

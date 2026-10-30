use std::time::Duration;
fn main() {
    let mode = std::env::var("HEALTH_PROBE_MODE").unwrap_or_else(|_| "success".into());
    match mode.as_str() {
        "success" => {
            println!(
                "CUDA Driver Version: 12.2\nFound 2 GPUs:\nGPU 0 memory allocation test passed.\nGPU 1 memory allocation test passed."
            );
            std::process::exit(23);
        }
        "allocation" => {
            println!("Found 1 GPUs:\nGPU 0 memory allocation test failed: out of memory")
        }
        "loader" => println!("Failed to initialize NVML"),
        "list" => println!("GPU 0: test device"),
        "empty" => {}
        "fallback_error" => {
            eprintln!("driver broken");
            std::process::exit(2);
        }
        "detected" => println!("NVIDIA-SMI"),
        "detection_error" => {
            println!("NVIDIA-SMI");
            std::process::exit(1);
        }
        "errors" => println!("Found 1 GPUs:\nerror one\nerror two\nerror three"),
        "nvcc" => println!("nvcc: compiler\nRelease 12.2, V12.2.140"),
        "nvcc_old" => println!("Release 11.7"),
        "smi_version" => println!("NVIDIA-SMI 565.57 Driver Version: 565.57 CUDA Version: 12.7 |"),
        "sleep" => {
            if let Some(path) = std::env::var_os("HEALTH_PROBE_STARTED") {
                std::fs::write(path, b"started").unwrap();
            }
            std::thread::sleep(Duration::from_secs(1));
            std::fs::write(
                std::env::var_os("HEALTH_PROBE_FINISHED").unwrap(),
                b"finished",
            )
            .unwrap();
        }
        _ => panic!("unknown child mode"),
    }
    std::process::exit(0);
}

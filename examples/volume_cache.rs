//! `cargo run --example volume_cache -- sync <local-directory> <mounted-volume>`
//! Replace sync with hydrate to restore the cache. Uses namespace "example".
use runpod::{
    Error,
    utils::{VolumeCache, VolumeCacheConfig},
};

fn main() -> runpod::Result<()> {
    let mut args = std::env::args_os().skip(1);
    let mode = args.next().and_then(|arg| arg.into_string().ok());
    let local = args
        .next()
        .ok_or_else(|| Error::InvalidInput("expected local directory and volume path".into()))?;
    let volume = args
        .next()
        .ok_or_else(|| Error::InvalidInput("expected volume path".into()))?;
    let cache = VolumeCache::new(
        [local],
        VolumeCacheConfig {
            namespace: Some("example".into()),
            volume_path: volume.into(),
            best_effort: false,
            ..Default::default()
        },
    )?;
    if !cache.available() {
        return Err(Error::InvalidInput(
            "volume directory is unavailable".into(),
        ));
    }
    let copied = match mode.as_deref() {
        Some("sync") => cache.sync_inline()?,
        Some("hydrate") => cache.hydrate()?,
        _ => return Err(Error::InvalidInput("expected sync or hydrate".into())),
    };
    println!("{copied}");
    Ok(())
}

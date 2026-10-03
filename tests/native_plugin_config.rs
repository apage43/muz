#![cfg(feature = "desktop")]
use muz::{assets::MemoryAssets, host::HostContext};
use std::sync::Arc;

#[test]
fn plugin_aliases_use_the_active_immutable_asset_resolver() {
    let config = muz::plugins::config_path();
    let mut a = MemoryAssets::default();
    a.insert(config.clone(), Arc::<[u8]>::from(&br#"{"fixture":{"path":"plugins/first.clap","version":"one"}}"#[..]), 1);
    let context_a = HostContext { assets: Arc::new(a), ..Default::default() };
    let mut b = MemoryAssets::default();
    b.insert(config.clone(), Arc::<[u8]>::from(&br#"{"fixture":{"path":"plugins/second.clap","version":"two"}}"#[..]), 2);
    let context_b = HostContext { assets: Arc::new(b), ..Default::default() };
    let first = context_a.run(|| muz::plugins::configured_alias("fixture").unwrap().unwrap());
    let second = context_b.run(|| muz::plugins::configured_alias("fixture").unwrap().unwrap());
    assert_eq!(first["version"], "one");
    assert_eq!(second["version"], "two");
    assert_eq!(first["path"], config.parent().unwrap().join("plugins/first.clap").display().to_string());
    assert_eq!(second["path"], config.parent().unwrap().join("plugins/second.clap").display().to_string());
    assert_eq!(context_a.run(|| muz::plugins::configured_alias("fixture").unwrap().unwrap()), first);
}

#[test]
fn absent_grants_and_cancelled_contexts_do_not_fall_back_to_disk_aliases() {
    let context = HostContext { assets: Arc::new(MemoryAssets::default()), ..Default::default() };
    assert!(context.run(|| muz::plugins::configured_alias("anything")).is_err());
    let mut assets = MemoryAssets::default();
    assets.insert(muz::plugins::config_path(), Arc::<[u8]>::from(&b"{}"[..]), 1);
    let context = HostContext { assets: Arc::new(assets), ..Default::default() };
    assert!(context.run(|| muz::plugins::configured_alias("anything")).unwrap().is_none());
    context.cancelled.store(true, std::sync::atomic::Ordering::Release);
    assert!(context.run(|| muz::plugins::configured_alias("anything")).is_err());
}

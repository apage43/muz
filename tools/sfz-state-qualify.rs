use std::{
    io::{self, BufRead},
    path::Path,
};
fn main() {
    let resolver: std::sync::Arc<dyn muz::assets::AssetResolver> = std::sync::Arc::new(
        muz::assets::ScopedFileAssets::configured_sfz().expect("configured SFZ asset grants"),
    );
    let context = muz::host::HostContext {
        assets: resolver.clone(),
        ..Default::default()
    };
    context.run(|| qualify(resolver));
}
fn qualify(resolver: std::sync::Arc<dyn muz::assets::AssetResolver>) {
    for line in io::stdin().lock().lines() {
        let v: serde_json::Value = serde_json::from_str(&line.unwrap()).unwrap();
        let path = v["path"].as_str().unwrap();
        let options = muz::sfz::Options {
            defines: serde_json::from_value(v["defines"].clone()).unwrap_or_default(),
            source_overlays: serde_json::from_value(v["source_overlays"].clone())
                .unwrap_or_default(),
            ..Default::default()
        };
        match muz::sfz::load(Path::new(path), &options) {
            Err(e) => println!(
                "{}",
                serde_json::json!({"path":path,"load_error":format!("{e:#}")})
            ),
            Ok(program) => {
                let normalized = serde_json::to_vec(&program).unwrap().len();
                let regions = program.regions.len();
                let value = serde_json::json!({"id":"state-probe","kind":"builtin.sfz","params":{},"sfz":{"path":path,"defines":options.defines,"source_overlays":options.source_overlays,"program":program,"max_voices":256,"max_sample_frames":muz::model::MAX_SFZ_SAMPLE_FRAMES,"seed":1,"embed_assets":false}});
                let device: muz::model::Device = serde_json::from_value(value).unwrap();
                let started = std::time::Instant::now();
                match muz::device_state::DeviceState::from_device(
                    muz::device_state::DeviceRole::Instrument,
                    device,
                ) {
                    Err(e) => println!(
                        "{}",
                        serde_json::json!({"path":path,"regions":regions,"normalized_program_bytes":normalized,"state_build_error":format!("{e:#}"),"seconds":started.elapsed().as_secs_f64()})
                    ),
                    Ok(state) => {
                        let source_bytes = state.source.len();
                        match state.encode() {
                            Err(e) => println!(
                                "{}",
                                serde_json::json!({"path":path,"regions":regions,"normalized_program_bytes":normalized,"source_bytes":source_bytes,"state_encode_error":format!("{e:#}"),"seconds":started.elapsed().as_secs_f64()})
                            ),
                            Ok(encoded) => {
                                let restored = muz::device_state::DeviceState::decode_with_resolver(
                                    &encoded,
                                    resolver.clone(),
                                );
                                let roundtrip = restored
                                    .as_ref()
                                    .is_ok_and(|r| r.device.sfz == state.device.sfz);
                                let error = restored.err().map(|e| format!("{e:#}"));
                                println!(
                                    "{}",
                                    serde_json::json!({"path":path,"regions":regions,"normalized_program_bytes":normalized,"source_bytes":source_bytes,"encoded_state_bytes":encoded.len(),"roundtrip":roundtrip,"decode_error":error,"linked_assets":state.linked_assets.len(),"seconds":started.elapsed().as_secs_f64()})
                                );
                            }
                        }
                    }
                }
            }
        }
    }
}

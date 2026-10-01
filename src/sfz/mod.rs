//! Bounded, off-thread SFZ preprocessing and hierarchy normalization.
pub mod opcodes;
use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
};
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SourceLocation {
    pub path: PathBuf,
    pub line: usize,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Diagnostic {
    pub source: SourceLocation,
    pub opcode: String,
    pub message: String,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Region {
    pub group_id: usize,
    pub master_id: usize,
    pub source: SourceLocation,
    pub sample: Option<PathBuf>,
    pub opcodes: BTreeMap<String, String>,
    pub opcode_sources: BTreeMap<String, SourceLocation>,
}
impl Region {
    pub fn number(&self, name: &str, default: f64) -> f64 {
        self.opcodes
            .get(name)
            .and_then(|v| v.parse().ok())
            .unwrap_or(default)
    }
    pub fn integer(&self, name: &str, default: i64) -> i64 {
        self.opcodes
            .get(name)
            .and_then(|v| v.parse().ok())
            .unwrap_or(default)
    }
    pub fn key(&self, name: &str, default: i32) -> i32 {
        self.opcodes
            .get(name)
            .and_then(|v| parse_key(v).ok())
            .unwrap_or(default)
    }
}
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Program {
    pub source: PathBuf,
    pub regions: Vec<Region>,
    pub curves: BTreeMap<u16, Vec<f32>>,
    pub controls: BTreeMap<u16, f32>,
    pub labels: BTreeMap<u16, String>,
    pub dependencies: Vec<PathBuf>,
    pub diagnostics: Vec<Diagnostic>,
}
impl Program {
    /// Validate normalized or deserialized input before any runtime compilation.
    /// Lexical coverage does not grant DSP support: the runtime checks that separately.
    pub fn validate(&self) -> Result<()> {
        for (n, v) in &self.controls {
            ensure!(
                *n < 128 && v.is_finite() && (0.0..=127.0).contains(v),
                "invalid initial CC {n}: {v}"
            );
        }
        for (id, values) in &self.curves {
            ensure!(
                values.len() == 128 && values.iter().all(|v| v.is_finite()),
                "invalid curve {id}"
            );
        }
        for region in &self.regions {
            let sample = region.opcodes.get("sample").with_context(|| {
                format!(
                    "{}:{}: region has no inherited sample",
                    region.source.path.display(),
                    region.source.line
                )
            })?;
            ensure!(
                (sample == "*silence") == region.sample.is_none(),
                "inconsistent normalized sample at {}:{}",
                region.source.path.display(),
                region.source.line
            );
            for (name, value) in &region.opcodes {
                let source = region.opcode_sources.get(name).unwrap_or(&region.source);
                let location = format!("{}:{}", source.path.display(), source.line);
                ensure!(
                    opcodes::recognized(name),
                    "{}:{}: unsupported opcode {name}",
                    region.source.path.display(),
                    region.source.line
                );
                let enums: Option<&[&str]> = match name.as_str() {
                    "trigger" => Some(&["attack", "release", "release_key", "first", "legato"]),
                    "loop_mode" => {
                        Some(&["no_loop", "one_shot", "loop_continuous", "loop_sustain"])
                    }
                    "off_mode" => Some(&["fast", "normal", "time"]),
                    "fil_type" | "fil2_type" => Some(&["lpf_1p", "lpf_2p", "hpf_1p", "hpf_2p"]),
                    "xf_cccurve" | "xf_keycurve" | "xf_velcurve" => Some(&["gain", "power"]),
                    n if n.starts_with("var") && n.ends_with("_mod") => Some(&["mult", "add"]),
                    _ => None,
                };
                if let Some(allowed) = enums {
                    ensure!(
                        allowed.contains(&value.as_str()),
                        "{location}: invalid {name} value {value}"
                    );
                    continue;
                }
                if name == "sample" || name == "default_path" || name.contains("label") {
                    continue;
                }
                if [
                    "key",
                    "lokey",
                    "hikey",
                    "pitch_keycenter",
                    "amp_keycenter",
                    "fil_keycenter",
                    "sw_lokey",
                    "sw_hikey",
                    "sw_last",
                    "sw_default",
                    "sw_down",
                    "sw_up",
                    "sw_previous",
                ]
                .contains(&name.as_str())
                {
                    let v =
                        parse_key(value).with_context(|| format!("{location}: {name}={value}"))?;
                    ensure!(
                        (-1..=127).contains(&v),
                        "{location}: invalid {name} key {value}"
                    );
                    continue;
                }
                let v = value
                    .parse::<f64>()
                    .with_context(|| format!("{location}: invalid numeric {name}={value}"))?;
                ensure!(v.is_finite(), "{location}: nonfinite {name}={value}");
                if ["lovel", "hivel", "lobend", "hibend"].contains(&name.as_str()) {
                    let (low, high) = if name.ends_with("bend") {
                        (-8192., 8192.)
                    } else {
                        (0., 127.)
                    };
                    ensure!(
                        (low..=high).contains(&v) && v.fract() == 0.,
                        "{location}: out of range {name}={value}"
                    );
                }
                if name.starts_with("locc") || name.starts_with("hicc") {
                    ensure!(
                        (0.0..=127.0).contains(&v),
                        "{location}: out of range {name}={value}"
                    );
                }
                if ["lorand", "hirand"].contains(&name.as_str()) {
                    ensure!(
                        (0.0..=1.0).contains(&v),
                        "{location}: out of range {name}={value}"
                    );
                }
                if ["seq_length", "seq_position", "polyphony"].contains(&name.as_str()) {
                    ensure!(
                        v >= 1. && v.fract() == 0.,
                        "{location}: invalid positive integer {name}={value}"
                    );
                }
                if name == "note_polyphony" {
                    ensure!(
                        v >= 0. && v.fract() == 0.,
                        "{location}: invalid nonnegative integer {name}={value}"
                    );
                }
                if ["group", "off_by", "offset", "loop_start", "loop_end"].contains(&name.as_str())
                {
                    ensure!(
                        v >= 0. && v.fract() == 0.,
                        "{location}: invalid nonnegative integer {name}={value}"
                    );
                }
                if name == "end" {
                    ensure!(
                        v >= -1. && v.fract() == 0.,
                        "{location}: invalid frame end={value}"
                    );
                }
            }
        }
        Ok(())
    }
}
/// An opt-in, original-byte-pinned removal of source tokens ignored by reference players.
/// This is not a general permissive parser mode or a sound-changing rewrite.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SourceOverlay {
    pub path: PathBuf,
    pub sha256: String,
    pub removals: Vec<String>,
    pub reason: String,
}
#[derive(Clone, Debug)]
pub struct Options {
    pub defines: BTreeMap<String, String>,
    pub source_overlays: Vec<SourceOverlay>,
    pub initial_controls: BTreeMap<u16, f32>,
    pub max_source_bytes: usize,
    pub max_include_depth: usize,
    pub max_regions: usize,
    pub strict: bool,
}
impl Default for Options {
    fn default() -> Self {
        Self {
            defines: BTreeMap::new(),
            source_overlays: vec![],
            initial_controls: BTreeMap::new(),
            max_source_bytes: 16 * 1024 * 1024,
            max_include_depth: 32,
            max_regions: 100_000,
            strict: true,
        }
    }
}
#[derive(Clone)]
struct Token {
    text: String,
    source: SourceLocation,
}
struct Reader<'a> {
    options: &'a Options,
    defines: BTreeMap<String, String>,
    stack: Vec<PathBuf>,
    dependencies: Vec<PathBuf>,
    bytes: usize,
    root_dir: PathBuf,
    diagnostics: Vec<Diagnostic>,
    applied_overlays: BTreeSet<usize>,
}
impl Reader<'_> {
    fn expand(&self, value: &str) -> Result<String> {
        let mut out = String::new();
        let mut chars = value.chars().peekable();
        while let Some(c) = chars.next() {
            if c != '$' {
                out.push(c);
                continue;
            }
            let mut key = String::new();
            while chars
                .peek()
                .is_some_and(|c| c.is_ascii_alphanumeric() || *c == '_')
            {
                key.push(chars.next().unwrap());
            }
            ensure!(!key.is_empty(), "empty macro");
            let val = self
                .defines
                .get(&key)
                .with_context(|| format!("undefined macro ${key}"))?;
            out.push_str(val);
        }
        Ok(out)
    }
    fn read(&mut self, path: &Path, out: &mut Vec<Token>) -> Result<()> {
        let path = crate::assets::resolve_sfz(&clean_path(path))
            .with_context(|| format!("resolving SFZ source/include {}", path.display()))?;
        ensure!(
            self.stack.len() < self.options.max_include_depth,
            "include depth exceeded: {}",
            path.display()
        );
        ensure!(
            !self.stack.contains(&path),
            "include cycle: {}",
            path.display()
        );
        self.stack.push(path.clone());
        if !self.dependencies.contains(&path) {
            self.dependencies.push(path.clone());
        }
        let snapshot = crate::assets::snapshot(
            &path,
            self.options.max_source_bytes.saturating_sub(self.bytes),
        )?;
        self.bytes += snapshot.bytes.len();
        let mut text = std::str::from_utf8(&snapshot.bytes)
            .context("SFZ source must be UTF-8")?
            .to_owned();
        for (overlay_index, overlay) in self.options.source_overlays.iter().enumerate() {
            let candidate = clean_path(&self.root_dir.join(&overlay.path));
            if crate::assets::resolve_sfz(&candidate).ok().as_ref() != Some(&path) {
                continue;
            }
            self.applied_overlays.insert(overlay_index);
            use sha2::{Digest, Sha256};
            let digest = format!("{:x}", Sha256::digest(&snapshot.bytes));
            ensure!(
                digest.eq_ignore_ascii_case(&overlay.sha256),
                "source overlay hash mismatch for {}",
                path.display()
            );
            ensure!(
                !overlay.reason.trim().is_empty() && !overlay.removals.is_empty(),
                "source overlay requires provenance reason/removals"
            );
            for removal in &overlay.removals {
                ensure!(
                    !removal.is_empty() && text.matches(removal.as_str()).count() == 1,
                    "source overlay token must occur exactly once in {}: {removal:?}",
                    path.display()
                );
                let line = text[..text.find(removal).unwrap()]
                    .bytes()
                    .filter(|b| *b == b'\n')
                    .count()
                    + 1;
                let replacement: String = removal
                    .chars()
                    .map(|c| if c == '\n' || c == '\r' { c } else { ' ' })
                    .collect();
                text = text.replacen(removal, &replacement, 1);
                self.diagnostics.push(Diagnostic {
                    source: SourceLocation {
                        path: path.clone(),
                        line,
                    },
                    opcode: "source_overlay".into(),
                    message: format!(
                        "pinned source token removed ({digest}): {:?}; {}",
                        removal, overlay.reason
                    ),
                });
            }
        }
        let text = text.trim_start_matches('\u{feff}');
        let mut block = false;
        for (index, line) in text.lines().enumerate() {
            let source = SourceLocation {
                path: path.clone(),
                line: index + 1,
            };
            let line = strip_comments(line, &mut block);
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            if let Some(rest) = line.strip_prefix("#define") {
                let mut fields = rest.trim().splitn(2, char::is_whitespace);
                let key = fields.next().unwrap_or("").trim_start_matches('$');
                ensure!(
                    !key.is_empty(),
                    "invalid define at {}:{}",
                    path.display(),
                    index + 1
                );
                let value = self.expand(fields.next().unwrap_or("").trim())?;
                self.defines.insert(key.into(), value);
                continue;
            }
            if let Some(rest) = line.strip_prefix("#include") {
                let name = self.expand(rest.trim().trim_matches('"'))?;
                let name = name.replace('\\', "/");
                let root_candidate = clean_path(&self.root_dir.join(&name));
                let local_candidate =
                    clean_path(&path.parent().unwrap_or(Path::new(".")).join(&name));
                let root_result = crate::assets::resolve_sfz(&root_candidate);
                let selected = match root_result {
                    Ok(root) => root,
                    Err(_) if root_candidate != local_candidate => {
                        match crate::assets::resolve_sfz(&local_candidate) {
                            Ok(local) => {
                                self.diagnostics.push(Diagnostic {
                                    source: source.clone(),
                                    opcode: "#include".into(),
                                    message: format!(
                                        "fragment-relative include fallback {name}: {}",
                                        local.display()
                                    ),
                                });
                                local
                            }
                            Err(_) => root_candidate,
                        }
                    }
                    Err(_) => root_candidate,
                };
                self.read(&selected, out).with_context(|| {
                    format!("{}:{}: including {name}", path.display(), index + 1)
                })?;
                continue;
            }
            ensure!(
                !line.starts_with('#'),
                "unsupported directive {}:{}: {line}",
                path.display(),
                index + 1
            );
            let expanded = self
                .expand(line)
                .with_context(|| format!("{}:{}", path.display(), index + 1))?;
            for text in
                tokenize(&expanded).with_context(|| format!("{}:{}", path.display(), index + 1))?
            {
                out.push(Token {
                    text,
                    source: source.clone(),
                });
            }
        }
        ensure!(!block, "unterminated comment: {}", path.display());
        self.stack.pop();
        Ok(())
    }
}
fn strip_comments(line: &str, block: &mut bool) -> String {
    let chars: Vec<char> = line.chars().collect();
    let mut out = String::new();
    let mut i = 0;
    let mut quote = false;
    while i < chars.len() {
        if *block {
            if chars.get(i..i + 2) == Some(&['*', '/']) {
                *block = false;
                i += 2
            } else {
                i += 1
            }
            continue;
        }
        if chars[i] == '"' {
            quote = !quote;
        }
        if !quote && chars.get(i..i + 2) == Some(&['/', '/']) {
            break;
        }
        if !quote && chars.get(i..i + 2) == Some(&['/', '*']) {
            *block = true;
            i += 2;
            continue;
        }
        out.push(chars[i]);
        i += 1;
    }
    out
}
fn tokenize(line: &str) -> Result<Vec<String>> {
    let bytes = line.as_bytes();
    let mut starts = Vec::new();
    let mut quoted = false;
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'"' {
            quoted = !quoted;
            i += 1;
            continue;
        }
        if !quoted
            && (bytes[i] == b'<'
                || ((i == 0 || bytes[i - 1].is_ascii_whitespace() || bytes[i - 1] == b'>')
                    && bytes[i].is_ascii_alphabetic()))
        {
            if bytes[i] == b'<' {
                starts.push(i);
                if let Some(end) = line[i..].find('>') {
                    i += end + 1;
                    continue;
                } else {
                    bail!("unterminated header")
                }
            }
            let mut end = i;
            while end < bytes.len() && (bytes[end].is_ascii_alphanumeric() || bytes[end] == b'_') {
                end += 1
            }
            if bytes.get(end) == Some(&b'=') {
                starts.push(i)
            }
        }
        i += 1;
    }
    ensure!(!quoted, "unterminated quoted value");
    ensure!(starts.first() == Some(&0), "invalid SFZ statement: {line}");
    let mut tokens = Vec::new();
    for (n, start) in starts.iter().enumerate() {
        let end = starts.get(n + 1).copied().unwrap_or(line.len());
        let token = line[*start..end].trim();
        if token.starts_with('<') {
            let close = token.find('>').unwrap();
            tokens.push(token[..=close].into());
            ensure!(
                token[close + 1..].trim().is_empty(),
                "invalid header suffix"
            );
        } else {
            tokens.push(token.into())
        }
    }
    Ok(tokens)
}
pub fn clean_path(path: &Path) -> PathBuf {
    let mut result = PathBuf::new();
    for component in path.components() {
        match component {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                if result.file_name().is_some_and(|s| s == "..")
                    || (!result.pop() && !result.has_root())
                {
                    result.push("..");
                }
            }
            _ => result.push(component.as_os_str()),
        }
    }
    result
}
pub fn parse_key(value: &str) -> Result<i32> {
    if let Ok(n) = value.parse::<i32>() {
        return Ok(n);
    }
    let v = value.to_ascii_lowercase();
    let bytes = v.as_bytes();
    ensure!(bytes.len() >= 2, "invalid key {value}");
    let pitch = match bytes[0] {
        b'c' => 0,
        b'd' => 2,
        b'e' => 4,
        b'f' => 5,
        b'g' => 7,
        b'a' => 9,
        b'b' => 11,
        _ => bail!("invalid key {value}"),
    };
    let (acc, start) = match bytes[1] {
        b'#' => (1, 2),
        b'b' => (-1, 2),
        _ => (0, 1),
    };
    let octave: i32 = v[start..].parse().context("invalid note octave")?;
    Ok((octave + 1) * 12 + pitch + acc)
}
pub fn load(path: &Path, options: &Options) -> Result<Program> {
    let resolved_root = crate::assets::resolve_sfz(&clean_path(path))?;
    let mut reader = Reader {
        options,
        root_dir: resolved_root.parent().unwrap_or(Path::new(".")).to_owned(),
        diagnostics: vec![],
        applied_overlays: BTreeSet::new(),
        defines: options
            .defines
            .iter()
            .map(|(k, v)| (k.trim_start_matches('$').into(), v.clone()))
            .collect(),
        stack: vec![],
        dependencies: vec![],
        bytes: 0,
    };
    let mut tokens = Vec::new();
    reader.read(path, &mut tokens)?;
    ensure!(
        reader.applied_overlays.len() == options.source_overlays.len(),
        "source overlay not present in selected program dependency closure"
    );
    let mut program = Program {
        source: resolved_root,
        dependencies: reader.dependencies,
        diagnostics: reader.diagnostics,
        ..Default::default()
    };
    let mut scopes: Vec<BTreeMap<String, String>> = vec![BTreeMap::new(); 4];
    let mut locations: Vec<BTreeMap<String, SourceLocation>> = vec![BTreeMap::new(); 4];
    let mut level = 0;
    let mut section = String::new();
    let mut default_path = PathBuf::new();
    let mut curve = BTreeMap::<String, String>::new();
    let mut region_source = None;
    let mut group_id = 0usize;
    let mut master_id = 0usize;
    fn finish(
        scopes: &[BTreeMap<String, String>],
        locations: &[BTreeMap<String, SourceLocation>],
        source: &mut Option<SourceLocation>,
        program: &mut Program,
        base: &Path,
        default_path: &Path,
        options: &Options,
        group_id: usize,
        master_id: usize,
    ) -> Result<()> {
        if let Some(source) = source.take() {
            ensure!(
                program.regions.len() < options.max_regions,
                "region limit exceeded"
            );
            let mut map = BTreeMap::new();
            for scope in scopes {
                map.extend(scope.clone())
            }
            let sample = map
                .get("sample")
                .filter(|s| s.as_str() != "*silence")
                .map(|s| {
                    clean_path(
                        &base
                            .join(default_path)
                            .join(s.trim_matches('"').replace('\\', "/")),
                    )
                });
            let sample = sample
                .map(|requested| -> Result<PathBuf> {
                    let resolved = crate::assets::resolve_sfz(&requested).with_context(|| {
                        format!(
                            "{}:{}: missing sample {}",
                            source.path.display(),
                            source.line,
                            requested.display()
                        )
                    })?;
                    if requested != resolved
                        && requested
                            .to_string_lossy()
                            .eq_ignore_ascii_case(&resolved.to_string_lossy())
                    {
                        program.diagnostics.push(Diagnostic {
                            source: source.clone(),
                            opcode: "sample".into(),
                            message: format!(
                                "case-resolved {} to {}",
                                requested.display(),
                                resolved.display()
                            ),
                        });
                    }
                    Ok(resolved)
                })
                .transpose()?;
            if let Some(p) = &sample {
                if !program.dependencies.contains(p) {
                    program.dependencies.push(p.clone());
                }
            }
            program.regions.push(Region {
                group_id,
                master_id,
                source,
                sample,
                opcodes: map,
                opcode_sources: locations.iter().flat_map(|s| s.clone()).collect(),
            });
        }
        Ok(())
    }
    fn finish_curve(curve: &mut BTreeMap<String, String>, program: &mut Program) -> Result<()> {
        if curve.is_empty() {
            return Ok(());
        }
        let id = curve
            .get("curve_index")
            .context("curve missing curve_index")?
            .parse()?;
        let mut values = vec![f32::NAN; 128];
        values[0] = 0.;
        values[127] = 1.;
        for (k, v) in curve.iter() {
            if let Some(n) = k.strip_prefix('v').and_then(|x| x.parse::<usize>().ok()) {
                ensure!(n < 128, "curve point out of range");
                values[n] = v.parse()?
            }
        }
        let points: Vec<(usize, f32)> = values
            .iter()
            .enumerate()
            .filter(|(_, v)| v.is_finite())
            .map(|(i, v)| (i, *v))
            .collect();
        for window in points.windows(2) {
            let (a, x) = window[0];
            let (b, y) = window[1];
            for (i, v) in values.iter_mut().enumerate().take(b + 1).skip(a) {
                *v = x + (y - x) * (i - a) as f32 / (b - a) as f32;
            }
        }
        program.curves.insert(id, values);
        curve.clear();
        Ok(())
    }
    let base = program.source.parent().unwrap_or(Path::new(".")).to_owned();
    for token in tokens {
        if token.text.starts_with('<') {
            finish(
                &scopes,
                &locations,
                &mut region_source,
                &mut program,
                &base,
                &default_path,
                options,
                group_id,
                master_id,
            )?;
            finish_curve(&mut curve, &mut program)?;
            section = token.text.trim_matches(['<', '>']).into();
            match section.as_str() {
                "global" => {
                    group_id += 1;
                    master_id += 1;
                    level = 0;
                    scopes.iter_mut().for_each(|s| s.clear());
                    locations.iter_mut().for_each(|s| s.clear());
                }
                "master" => {
                    master_id += 1;
                    group_id += 1;
                    level = 1;
                    scopes[1..].iter_mut().for_each(|s| s.clear());
                    locations[1..].iter_mut().for_each(|s| s.clear());
                }
                "group" => {
                    group_id += 1;
                    level = 2;
                    scopes[2..].iter_mut().for_each(|s| s.clear());
                    locations[2..].iter_mut().for_each(|s| s.clear());
                }
                "region" => {
                    level = 3;
                    scopes[3].clear();
                    locations[3].clear();
                    region_source = Some(token.source);
                }
                "control" | "curve" => {}
                _ => bail!("unsupported section <{section}>"),
            };
            continue;
        }
        let (name, value) = token.text.split_once('=').context("missing opcode value")?;
        let canonical_name = name.trim().to_ascii_lowercase();
        let name = canonical_name.as_str();
        let value = value.trim().trim_matches('"');
        if name == "note_polyphony" && value.parse::<f64>().ok() == Some(0.) {
            program.diagnostics.push(Diagnostic {source:token.source.clone(),opcode:name.into(),message:"note_polyphony=0 playback clamps to minimum 1, matching sfizz reference behavior".into()});
        }
        if !opcodes::recognized(name)
            && name != "sample"
            && name != "curve_index"
            && !name
                .strip_prefix('v')
                .is_some_and(|s| s.parse::<u16>().is_ok())
        {
            let diagnostic = Diagnostic {
                source: token.source.clone(),
                opcode: name.into(),
                message: "unrecognized opcode; no playback semantics registered".into(),
            };
            ensure!(
                !options.strict,
                "{}:{}: unsupported opcode {name}",
                token.source.path.display(),
                token.source.line
            );
            program.diagnostics.push(diagnostic);
        }
        match section.as_str() {
            "control" => {
                if name == "default_path" {
                    default_path = PathBuf::from(value.replace('\\', "/"));
                } else if let Some(n) = name.strip_prefix("set_cc") {
                    program.controls.insert(n.parse()?, value.parse()?);
                } else if let Some(n) = name.strip_prefix("set_hdcc") {
                    program
                        .controls
                        .insert(n.parse()?, value.parse::<f32>()? * 127.);
                } else if let Some(n) = name.strip_prefix("label_cc") {
                    program.labels.insert(n.parse()?, value.into());
                }
            }
            "curve" => {
                curve.insert(name.into(), value.into());
            }
            "global" | "master" | "group" | "region" => {
                scopes[level].insert(name.into(), value.into());
                locations[level].insert(name.into(), token.source.clone());
            }
            _ => bail!("opcode outside section"),
        }
    }
    finish(
        &scopes,
        &locations,
        &mut region_source,
        &mut program,
        &base,
        &default_path,
        options,
        group_id,
        master_id,
    )?;
    finish_curve(&mut curve, &mut program)?;
    program.controls.extend(options.initial_controls.clone());
    if options.strict {
        program.validate()?;
    }
    Ok(program)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn keys() {
        assert_eq!(parse_key("c4").unwrap(), 60);
        assert_eq!(parse_key("Db3").unwrap(), 49);
        assert_eq!(parse_key("-1").unwrap(), -1);
    }
    #[test]
    fn lexer() {
        assert_eq!(
            tokenize("<region> sample=My file.wav key=c4").unwrap(),
            vec!["<region>", "sample=My file.wav", "key=c4"]
        );
    }
    #[test]
    fn hierarchy_includes() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("fragment.sfz"),
            "<region> sample=$file key=c4\n",
        )
        .unwrap();
        let root = dir.path().join("main.sfz");
        std::fs::create_dir(dir.path().join("Samples")).unwrap();
        std::fs::write(dir.path().join("Samples/Mixed Case.wav"), b"sample").unwrap();
        std::fs::write(&root,"#define $file Mixed Case.wav\n<control> default_path=Samples/\n<global> volume=-4\n<master> pan=20\n<group> lovel=50\n#include \"fragment.sfz\"\n<group> lovel=1\n<region> sample=*silence\n").unwrap();
        let p = load(&root, &Options::default()).unwrap();
        assert_eq!(p.regions.len(), 2);
        assert_eq!(p.regions[0].number("volume", 0.), -4.);
        assert_eq!(p.regions[1].integer("lovel", 0), 1);
        assert!(
            p.regions[0]
                .sample
                .as_ref()
                .unwrap()
                .ends_with("Samples/Mixed Case.wav")
        );
        assert!(p.regions[1].sample.is_none());
    }
    #[test]
    fn repeated_include_order_curves_and_overrides() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("part.sfz"),
            "<region> sample=*silence volume=$gain\n",
        )
        .unwrap();
        let root = dir.path().join("main.sfz");
        std::fs::write(&root,"<control> set_cc1=20 set_hdcc2=0.5\n#define $gain -3\n#include \"part.sfz\"\n#define $gain -9\n#include \"part.sfz\"\n<curve>curve_index=7 v000=0 v064=0.25 v127=1\n").unwrap();
        let p = load(
            &root,
            &Options {
                initial_controls: BTreeMap::from([(1, 100.)]),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(p.regions[0].number("volume", 0.), -3.);
        assert_eq!(p.regions[1].number("volume", 0.), -9.);
        assert_eq!(p.controls[&1], 100.);
        assert_eq!(p.controls[&2], 63.5);
        assert_eq!(p.curves[&7][32], 0.125);
        let encoded = serde_json::to_string(&p).unwrap();
        assert_eq!(serde_json::from_str::<Program>(&encoded).unwrap(), p);
    }
    #[test]
    fn strict_diagnostics_and_bounds() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("main.sfz");
        std::fs::write(&root, "<region> sample=*silence magic_opcode=3").unwrap();
        assert!(load(&root, &Options::default()).is_err());
        let p = load(
            &root,
            &Options {
                strict: false,
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(p.diagnostics[0].opcode, "magic_opcode");
        assert!(
            load(
                &root,
                &Options {
                    strict: false,
                    max_regions: 0,
                    ..Default::default()
                }
            )
            .is_err()
        );
        assert!(
            load(
                &root,
                &Options {
                    max_source_bytes: 2,
                    ..Default::default()
                }
            )
            .is_err()
        );
    }
    #[test]
    fn validates_deserialized_numeric_and_enum_values() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("main.sfz");
        for value in [
            "ampeg_attack=NaN",
            "volume=garbage",
            "trigger=unknown",
            "fil_type=magic",
            "key=c99",
        ] {
            std::fs::write(&root, format!("<global> {value}\n<region> sample=*silence")).unwrap();
            assert!(load(&root, &Options::default()).is_err(), "{value}");
        }
        std::fs::write(&root, "<region> sample=*silence volume=-3").unwrap();
        let mut p = load(&root, &Options::default()).unwrap();
        p.regions[0]
            .opcodes
            .insert("unknown_operator".into(), "1".into());
        let p: Program = serde_json::from_str(&serde_json::to_string(&p).unwrap()).unwrap();
        assert!(p.validate().is_err());
    }
    #[test]
    fn opcode_macros_expand_before_lexing() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("main.sfz");
        std::fs::write(&root,"#define $RES 98\n<control> label_cc$RES=Resonance set_cc$RES=50\n<region> sample=*silence Key=C4").unwrap();
        let p = load(&root, &Options::default()).unwrap();
        assert_eq!(p.labels[&98], "Resonance");
        assert_eq!(p.controls[&98], 50.);
        assert_eq!(p.regions[0].key("key", 0), 60);
    }
    #[test]
    fn sample_case_fallback_reports_resolved_identity() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("Samples")).unwrap();
        std::fs::write(dir.path().join("Samples/Tone.WAV"), b"sample").unwrap();
        std::fs::write(
            dir.path().join("Part.SFZ"),
            "<region> sample=samples/tone.wav",
        )
        .unwrap();
        let root = dir.path().join("main.sfz");
        std::fs::write(&root, "#include \"part.sfz\"").unwrap();
        let p = load(&root, &Options::default()).unwrap();
        assert!(
            p.regions[0]
                .sample
                .as_ref()
                .unwrap()
                .ends_with("Samples/Tone.WAV")
        );
        assert!(
            p.diagnostics
                .iter()
                .any(|d| d.message.contains("case-resolved"))
        );
    }
    #[test]
    fn includes_prefer_root_and_warn_on_fragment_fallback() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("maps")).unwrap();
        std::fs::write(dir.path().join("main.sfz"), "#include \"maps/a.sfz\"").unwrap();
        std::fs::write(dir.path().join("maps/a.sfz"), "#include \"shared.sfz\"").unwrap();
        std::fs::write(
            dir.path().join("shared.sfz"),
            "<region> sample=*silence volume=-1",
        )
        .unwrap();
        std::fs::write(
            dir.path().join("maps/shared.sfz"),
            "<region> sample=*silence volume=-2",
        )
        .unwrap();
        let root = dir.path().join("main.sfz");
        let p = load(&root, &Options::default()).unwrap();
        assert_eq!(p.regions[0].number("volume", 0.), -1.);
        assert!(p.diagnostics.is_empty());
        std::fs::remove_file(dir.path().join("shared.sfz")).unwrap();
        let p = load(&root, &Options::default()).unwrap();
        assert_eq!(p.regions[0].number("volume", 0.), -2.);
        assert!(
            p.diagnostics
                .iter()
                .any(|d| d.message.contains("fragment-relative"))
        );
    }
    #[test]
    fn source_removals_are_explicit_hash_pinned_and_keep_line_numbers() {
        use sha2::{Digest, Sha256};
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("main.sfz");
        let text = "/ Bad comment\n<group>\nvolume-1\n<region> sample=*silence\n";
        std::fs::write(&root, text).unwrap();
        assert!(load(&root, &Options::default()).is_err());
        let overlay = SourceOverlay {
            path: "main.sfz".into(),
            sha256: format!("{:x}", Sha256::digest(text.as_bytes())),
            removals: vec!["/ Bad comment\n".into(), "volume-1\n".into()],
            reason: "reference ignores malformed tokens with diagnostics".into(),
        };
        let options = Options {
            source_overlays: vec![overlay],
            ..Default::default()
        };
        let p = load(&root, &options).unwrap();
        assert_eq!(p.regions[0].source.line, 4);
        assert_eq!(p.regions[0].number("volume", 0.), 0.);
        assert_eq!(p.diagnostics.len(), 2);
        std::fs::write(&root, format!("{text}\n")).unwrap();
        assert!(format!("{:#}", load(&root, &options).unwrap_err()).contains("hash mismatch"));
    }
    #[test]
    fn macro_expanded_numeric_route_families_and_zero_polyphony() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("main.sfz");
        std::fs::write(&root,"#define $VELTRACK 99\n<group> amp_veltrack_oncc$VELTRACK=-100 note_polyphony=0\n<region> sample=*silence").unwrap();
        let p = load(&root, &Options::default()).unwrap();
        assert_eq!(p.regions[0].number("amp_veltrack_oncc99", 0.), -100.);
        assert_eq!(p.regions[0].integer("note_polyphony", 99), 0);
        assert!(p.diagnostics.iter().any(|d| d.opcode == "note_polyphony"));
    }
    #[test]
    fn txt_fragments_retain_region_label_metadata() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("main.sfz");
        std::fs::write(
            dir.path().join("PP.txt"),
            "<region> region_label=Grand Piano Pianissimo sample=*silence",
        )
        .unwrap();
        std::fs::write(&root, "#include \"PP.txt\"").unwrap();
        let p = load(&root, &Options::default()).unwrap();
        assert_eq!(
            p.regions[0].opcodes["region_label"],
            "Grand Piano Pianissimo"
        );
        assert!(p.dependencies.iter().any(|p| p.ends_with("PP.txt")));
    }
    #[test]
    fn cycles_and_undefined() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("main.sfz");
        std::fs::write(&root, "#include \"main.sfz\"").unwrap();
        assert!(format!("{:#}", load(&root, &Options::default()).unwrap_err()).contains("cycle"));
        std::fs::write(&root, "<region> sample=$missing").unwrap();
        assert!(load(&root, &Options::default()).is_err());
    }
}

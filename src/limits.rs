//! Explicit composition limits, separate from realtime scheduling capacity.
use crate::music::Pattern;
use anyhow::{Result, bail};

#[derive(Clone, Copy, Debug)]
pub struct ExpansionLimits {
    pub notes: usize,
    pub controls: usize,
    pub raw: usize,
    pub bytes: usize,
}
impl Default for ExpansionLimits {
    fn default() -> Self {
        Self {
            notes: 200_000,
            controls: 200_000,
            raw: 200_000,
            bytes: 256 * 1024 * 1024,
        }
    }
}
#[derive(Clone, Copy, Debug, Default)]
pub struct ExpansionCost {
    pub notes: usize,
    pub controls: usize,
    pub raw: usize,
    pub bytes: usize,
}
fn add(a: usize, b: usize) -> Result<usize> {
    a.checked_add(b)
        .ok_or_else(|| anyhow::anyhow!("expansion cost overflow"))
}
pub(crate) fn json_bytes(v: &serde_json::Value) -> Result<usize> {
    crate::host::check_cancelled()?;
    let mut bytes = std::mem::size_of::<serde_json::Value>();
    match v {
        serde_json::Value::String(s) => bytes = add(bytes, s.len())?,
        serde_json::Value::Array(a) => {
            for v in a {
                bytes = add(bytes, json_bytes(v)?)?;
            }
        }
        serde_json::Value::Object(r) => {
            for (k, v) in r {
                bytes = add(add(bytes, k.len())?, json_bytes(v)?)?;
            }
        }
        _ => {}
    }
    Ok(bytes)
}
impl ExpansionCost {
    pub fn note(n: &crate::music::Note) -> Result<Self> {
        crate::host::check_cancelled()?;
        let mut bytes = std::mem::size_of_val(n);
        for text in [&n.key, &n.voice]
            .into_iter()
            .chain(n.tags.iter())
            .chain(n.hand.iter())
        {
            bytes = add(bytes, text.len())?;
        }
        for (k, v) in &n.data {
            bytes = add(add(bytes, k.len())?, json_bytes(v)?)?;
        }
        Ok(Self {
            notes: 1,
            bytes,
            ..Default::default()
        })
    }
    pub fn of(p: &Pattern) -> Result<Self> {
        let mut cost = Self {
            notes: p.notes.len(),
            controls: p.controls.len(),
            raw: p.raw.len(),
            bytes: 0,
        };
        for n in &p.notes {
            crate::host::check_cancelled()?;
            cost.bytes = add(cost.bytes, std::mem::size_of_val(n))?;
            for text in [&n.key, &n.voice]
                .into_iter()
                .chain(n.tags.iter())
                .chain(n.hand.iter())
            {
                cost.bytes = add(cost.bytes, text.len())?;
            }
            for (k, v) in &n.data {
                cost.bytes = add(add(cost.bytes, k.len())?, json_bytes(v)?)?;
            }
        }
        for c in &p.controls {
            cost.bytes = add(cost.bytes, std::mem::size_of_val(c))?;
        }
        for r in &p.raw {
            cost.bytes = add(add(cost.bytes, std::mem::size_of_val(r))?, r.bytes.len())?;
        }
        Ok(cost)
    }
    pub fn plus(self, other: Self) -> Result<Self> {
        Ok(Self {
            notes: add(self.notes, other.notes)?,
            controls: add(self.controls, other.controls)?,
            raw: add(self.raw, other.raw)?,
            bytes: add(self.bytes, other.bytes)?,
        })
    }
    pub fn repeated(self, count: usize, prefix: usize) -> Result<Self> {
        let mul = |v: usize| {
            v.checked_mul(count)
                .ok_or_else(|| anyhow::anyhow!("expansion cost overflow"))
        };
        Ok(Self {
            notes: mul(self.notes)?,
            controls: mul(self.controls)?,
            raw: mul(self.raw)?,
            bytes: mul(add(
                self.bytes,
                self.notes
                    .checked_mul(prefix)
                    .ok_or_else(|| anyhow::anyhow!("expansion cost overflow"))?,
            )?)?,
        })
    }
    pub fn check(self, limit: ExpansionLimits) -> Result<Self> {
        crate::host::check_cancelled()?;
        for (name, actual, max) in [
            ("notes", self.notes, limit.notes),
            ("controls", self.controls, limit.controls),
            ("raw events", self.raw, limit.raw),
            ("logical bytes", self.bytes, limit.bytes),
        ] {
            if actual > max {
                bail!("expansion requires {actual} {name}; budget is {max}");
            }
        }
        Ok(self)
    }
}

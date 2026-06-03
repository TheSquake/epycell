//! Minimal .ipynb (nbformat v4) load/save.
//!
//! We only care about cell type + source for now — stored outputs are ignored
//! on load (cells re-run live) and written empty on save.

use std::path::Path;

use anyhow::{Context, Result};
use serde_json::{json, Value};

/// A cell as loaded from / saved to disk.
pub struct NbCell {
    pub source: String,
    pub markdown: bool,
}

/// nbformat stores `source` as either a string or an array of line-strings.
fn source_to_string(v: &Value) -> String {
    match v {
        Value::Array(lines) => lines.iter().filter_map(|x| x.as_str()).collect(),
        Value::String(s) => s.clone(),
        _ => String::new(),
    }
}

/// Load code + markdown cells from a .ipynb file.
pub fn load(path: &Path) -> Result<Vec<NbCell>> {
    let text = std::fs::read_to_string(path)
        .with_context(|| format!("reading {}", path.display()))?;
    let nb: Value = serde_json::from_str(&text).context("parsing .ipynb json")?;
    let cells = nb
        .get("cells")
        .and_then(Value::as_array)
        .context("not a notebook: missing `cells` array")?;

    let mut out = Vec::new();
    for c in cells {
        let kind = c.get("cell_type").and_then(Value::as_str).unwrap_or("code");
        if kind != "code" && kind != "markdown" {
            continue; // skip raw cells
        }
        let source = c.get("source").map(source_to_string).unwrap_or_default();
        out.push(NbCell {
            source,
            markdown: kind == "markdown",
        });
    }
    Ok(out)
}

/// Save cells to a .ipynb file (nbformat 4.5), outputs cleared.
pub fn save(path: &Path, cells: &[NbCell]) -> Result<()> {
    let arr: Vec<Value> = cells
        .iter()
        .map(|c| {
            if c.markdown {
                json!({
                    "cell_type": "markdown",
                    "metadata": {},
                    "source": c.source,
                })
            } else {
                json!({
                    "cell_type": "code",
                    "metadata": {},
                    "execution_count": Value::Null,
                    "outputs": [],
                    "source": c.source,
                })
            }
        })
        .collect();

    let nb = json!({
        "cells": arr,
        "metadata": {
            "kernelspec": { "name": "python3", "display_name": "Python 3" },
            "language_info": { "name": "python" }
        },
        "nbformat": 4,
        "nbformat_minor": 5,
    });

    std::fs::write(path, serde_json::to_string_pretty(&nb)?)
        .with_context(|| format!("writing {}", path.display()))?;
    Ok(())
}

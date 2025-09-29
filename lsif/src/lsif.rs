use anyhow::{anyhow, Context, Result};
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::sync::{Mutex, OnceLock};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct Pos {
    line: u32,
    character: u32,
}

#[derive(Clone, Copy, Debug)]
struct Span {
    start: Pos,
    end: Pos,
}

fn pos_leq(a: Pos, b: Pos) -> bool {
    a.line < b.line || (a.line == b.line && a.character <= b.character)
}
fn pos_lt(a: Pos, b: Pos) -> bool {
    a.line < b.line || (a.line == b.line && a.character < b.character)
}
fn contains(span: Span, p: Pos) -> bool {
    pos_leq(span.start, p) && pos_lt(p, span.end)
}

pub struct LSIFIndex {
    // vertices
    documents: HashMap<i64, String>,  // id -> uri
    doc_by_uri: HashMap<String, i64>, // uri -> id
    ranges: HashMap<i64, Span>,       // id -> span
    range_doc: HashMap<i64, i64>,     // range id -> doc id
    result_sets: HashSet<i64>,        // ids that are resultSet vertices
    // edges
    range_to_resultset: HashMap<i64, i64>, // range id -> resultSet id
    rset_to_def: HashMap<i64, i64>,        // resultSet id -> definitionResult id
    rset_to_ref: HashMap<i64, i64>,        // resultSet id -> referenceResult id
    range_to_def: HashMap<i64, i64>,       // fallback: range id -> definitionResult id
    range_to_ref: HashMap<i64, i64>,       // fallback: range id -> referenceResult id
    // results
    def_items: HashMap<i64, Vec<i64>>, // definitionResult id -> [range ids]
    ref_items: HashMap<i64, RefItems>, // referenceResult id -> split items
    hover_results: HashMap<i64, Value>, // hoverResult id -> result payload
}

#[derive(Default)]
struct RefItems {
    definitions: Vec<i64>,
    references: Vec<i64>,
    declarations: Vec<i64>,
}

impl LSIFIndex {
    fn new() -> Self {
        Self {
            documents: HashMap::new(),
            doc_by_uri: HashMap::new(),
            ranges: HashMap::new(),
            range_doc: HashMap::new(),
            result_sets: HashSet::new(),
            range_to_resultset: HashMap::new(),
            rset_to_def: HashMap::new(),
            rset_to_ref: HashMap::new(),
            range_to_def: HashMap::new(),
            range_to_ref: HashMap::new(),
            def_items: HashMap::new(),
            ref_items: HashMap::new(),
            hover_results: HashMap::new(),
        }
    }

    fn add_vertex(&mut self, v: &serde_json::Map<String, Value>) {
        if let Some(Value::String(label)) = v.get("label") {
            match label.as_str() {
                "document" => {
                    if let (Some(Value::Number(idv)), Some(Value::String(uri))) =
                        (v.get("id"), v.get("uri"))
                    {
                        if let Some(id) = idv.as_i64() {
                            self.documents.insert(id, uri.clone());
                            self.doc_by_uri.insert(uri.clone(), id);
                        }
                    }
                }
                "range" => {
                    if let Some(Value::Number(idv)) = v.get("id") {
                        if let Some(id) = idv.as_i64() {
                            let start = v.get("start");
                            let end = v.get("end");
                            if let (Some(Value::Object(s)), Some(Value::Object(e))) = (start, end) {
                                let span = Span {
                                    start: Pos {
                                        line: s.get("line").and_then(|x| x.as_u64()).unwrap_or(0)
                                            as u32,
                                        character: s
                                            .get("character")
                                            .and_then(|x| x.as_u64())
                                            .unwrap_or(0)
                                            as u32,
                                    },
                                    end: Pos {
                                        line: e.get("line").and_then(|x| x.as_u64()).unwrap_or(0)
                                            as u32,
                                        character: e
                                            .get("character")
                                            .and_then(|x| x.as_u64())
                                            .unwrap_or(0)
                                            as u32,
                                    },
                                };
                                self.ranges.insert(id, span);
                            }
                        }
                    }
                }
                "resultSet" => {
                    if let Some(Value::Number(idv)) = v.get("id") {
                        if let Some(id) = idv.as_i64() {
                            self.result_sets.insert(id);
                        }
                    }
                }
                "hoverResult" => {
                    if let Some(Value::Number(idv)) = v.get("id") {
                        if let Some(id) = idv.as_i64() {
                            if let Some(res) = v.get("result").cloned() {
                                self.hover_results.insert(id, res);
                            }
                        }
                    }
                }
                _ => {}
            }
        }
    }

    fn add_edge(&mut self, e: &serde_json::Map<String, Value>) {
        let label = match e.get("label").and_then(|v| v.as_str()) {
            Some(s) => s,
            None => return,
        };
        match label {
            "contains" => {
                let out = e.get("outV").and_then(|v| v.as_i64());
                if let Some(doc_id) = out {
                    if self.documents.contains_key(&doc_id) {
                        if let Some(Value::Array(invs)) = e.get("inVs") {
                            for iv in invs {
                                if let Some(rid) = iv.as_i64() {
                                    self.range_doc.insert(rid, doc_id);
                                }
                            }
                        }
                    }
                }
            }
            "next" => {
                if let (Some(ov), Some(iv)) = (
                    e.get("outV").and_then(|v| v.as_i64()),
                    e.get("inV").and_then(|v| v.as_i64()),
                ) {
                    self.range_to_resultset.insert(ov, iv);
                }
            }
            "textDocument/definition" => {
                if let (Some(ov), Some(iv)) = (
                    e.get("outV").and_then(|v| v.as_i64()),
                    e.get("inV").and_then(|v| v.as_i64()),
                ) {
                    if self.result_sets.contains(&ov) {
                        self.rset_to_def.insert(ov, iv);
                    } else {
                        self.range_to_def.insert(ov, iv);
                    }
                }
            }
            "textDocument/references" => {
                if let (Some(ov), Some(iv)) = (
                    e.get("outV").and_then(|v| v.as_i64()),
                    e.get("inV").and_then(|v| v.as_i64()),
                ) {
                    if self.result_sets.contains(&ov) {
                        self.rset_to_ref.insert(ov, iv);
                    } else {
                        self.range_to_ref.insert(ov, iv);
                    }
                }
            }
            "textDocument/hover" => {
                // Note: minimal ingester doesn't wire hover edges; extend if needed.
                let _ = e; // silence unused warning if not used
            }
            "item" => {
                let outv = e.get("outV").and_then(|v| v.as_i64());
                if let Some(out) = outv {
                    let mut targets: Vec<i64> = Vec::new();
                    if let Some(Value::Array(invs)) = e.get("inVs") {
                        for iv in invs {
                            if let Some(rid) = iv.as_i64() {
                                targets.push(rid);
                            }
                        }
                    }
                    let prop = e.get("property").and_then(|v| v.as_str());
                    if let Some(p) = prop {
                        let entry = self.ref_items.entry(out).or_default();
                        match p {
                            "definitions" => entry.definitions.extend(targets),
                            "references" => entry.references.extend(targets),
                            "declarations" => entry.declarations.extend(targets),
                            _ => {}
                        }
                    } else {
                        self.def_items.entry(out).or_default().extend(targets);
                    }
                }
            }
            _ => {}
        }
    }

    fn finalize(&mut self) {}

    fn find_best_range(&self, uri: &str, pos: Pos) -> Option<i64> {
        let did = *self.doc_by_uri.get(uri)?;
        let mut best: Option<(i64, Span)> = None;
        for (rid, span) in self.ranges.iter() {
            if let Some(doc_id) = self.range_doc.get(rid) {
                if *doc_id == did && contains(*span, pos) {
                    let cur = *span;
                    match best {
                        None => best = Some((*rid, cur)),
                        Some((_, prev)) => {
                            let prev_len = (prev.end.line - prev.start.line) as i64 * 1_000_000
                                + (prev.end.character - prev.start.character) as i64;
                            let cur_len = (cur.end.line - cur.start.line) as i64 * 1_000_000
                                + (cur.end.character - cur.start.character) as i64;
                            if cur_len < prev_len {
                                best = Some((*rid, cur));
                            }
                        }
                    }
                }
            }
        }
        best.map(|(rid, _)| rid)
    }

    fn resultset_for_range(&self, rid: i64) -> Option<i64> {
        self.range_to_resultset
            .get(&rid)
            .copied()
            .filter(|id| self.result_sets.contains(id))
    }

    fn ranges_for_result(&self, res_id: i64) -> Vec<(String, Span)> {
        let mut out = Vec::new();
        if let Some(ids) = self.def_items.get(&res_id) {
            for rid in ids {
                if let (Some(span), Some(doc_id)) = (self.ranges.get(rid), self.range_doc.get(rid))
                {
                    if let Some(uri) = self.documents.get(doc_id) {
                        out.push((uri.clone(), *span));
                    }
                }
            }
        }
        out
    }

    fn ranges_for_refs(&self, res_id: i64, include_decls: bool) -> Vec<(String, Span)> {
        let mut out = Vec::new();
        if let Some(items) = self.ref_items.get(&res_id) {
            let mut push_ids = |ids: &Vec<i64>| {
                for rid in ids {
                    if let (Some(span), Some(doc_id)) =
                        (self.ranges.get(rid), self.range_doc.get(rid))
                    {
                        if let Some(uri) = self.documents.get(doc_id) {
                            out.push((uri.clone(), *span));
                        }
                    }
                }
            };
            push_ids(&items.references);
            if include_decls {
                push_ids(&items.definitions);
                push_ids(&items.declarations);
            }
        }
        out
    }
}

static LSIF: OnceLock<Mutex<LSIFIndex>> = OnceLock::new();

fn with_index<F, T>(f: F) -> Result<T>
where
    F: FnOnce(&mut LSIFIndex) -> Result<T>,
{
    let m = LSIF.get_or_init(|| Mutex::new(LSIFIndex::new()));
    let mut guard = m.lock().map_err(|_| anyhow!("LSIF index poisoned"))?;
    f(&mut guard)
}

pub fn load_from_path(path: &str) -> Result<()> {
    with_index(|idx| {
        *idx = LSIFIndex::new();
        let file = File::open(path).with_context(|| format!("open LSIF: {}", path))?;
        let reader = BufReader::new(file);
        for line in reader.lines() {
            let line = line?;
            if line.trim().is_empty() {
                continue;
            }
            let v: Value = match serde_json::from_str(&line) {
                Ok(v) => v,
                Err(_) => continue,
            };
            if let Value::Object(map) = v {
                match map.get("type").and_then(|t| t.as_str()) {
                    Some("vertex") => idx.add_vertex(&map),
                    Some("edge") => idx.add_edge(&map),
                    _ => {}
                }
            }
        }
        idx.finalize();
        Ok(())
    })
}

fn loc_json(uri: &str, span: Span) -> Value {
    json!({
        "uri": uri,
        "range": {
            "start": {"line": span.start.line, "character": span.start.character},
            "end": {"line": span.end.line, "character": span.end.character}
        }
    })
}

pub fn query_definition(uri: &str, line: u32, character: u32) -> Result<Value> {
    with_index(|idx| {
        let pos = Pos { line, character };
        let rid = idx
            .find_best_range(uri, pos)
            .ok_or_else(|| anyhow!("no LSIF range at position"))?;
        let rset = idx.resultset_for_range(rid);
        let def_res = rset
            .and_then(|rs| idx.rset_to_def.get(&rs).copied())
            .or_else(|| idx.range_to_def.get(&rid).copied());
        let ranges: Vec<(String, Span)> = if let Some(def_id) = def_res {
            idx.ranges_for_result(def_id)
        } else if let Some(ref_id) = rset
            .and_then(|rs| idx.rset_to_ref.get(&rs).copied())
            .or_else(|| idx.range_to_ref.get(&rid).copied())
        {
            idx.ranges_for_refs(ref_id, true)
        } else {
            Vec::new()
        };
        Ok(
            json!({ "locations": ranges.into_iter().map(|(u,s)| loc_json(&u, s)).collect::<Vec<_>>() }),
        )
    })
}

pub fn query_references(
    uri: &str,
    line: u32,
    character: u32,
    include_declarations: bool,
) -> Result<Value> {
    with_index(|idx| {
        let pos = Pos { line, character };
        let rid = idx
            .find_best_range(uri, pos)
            .ok_or_else(|| anyhow!("no LSIF range at position"))?;
        let rset = idx.resultset_for_range(rid);
        let ref_res = rset
            .and_then(|rs| idx.rset_to_ref.get(&rs).copied())
            .or_else(|| idx.range_to_ref.get(&rid).copied())
            .ok_or_else(|| anyhow!("no references for symbol"))?;
        let ranges = idx.ranges_for_refs(ref_res, include_declarations);
        Ok(
            json!({ "locations": ranges.into_iter().map(|(u,s)| loc_json(&u, s)).collect::<Vec<_>>() }),
        )
    })
}

pub fn query_hover(uri: &str, line: u32, character: u32) -> Result<Value> {
    let _ = (uri, line, character);
    Err(anyhow!("hover not available in minimal ingester"))
}

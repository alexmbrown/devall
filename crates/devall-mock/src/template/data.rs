//! `dataSource`, `dataRows` and `dataCount`: lookups into `mocks/data/` (see [`crate::data`]).

use std::sync::Arc;

use handlebars::{Context, Handlebars, Helper, HelperDef, HelperResult, Output, RenderContext, RenderError, ScopedJson};
use serde_json::{Value, json};

use super::helpers::{Args, Mode, collect, error_text, render_value};
use crate::data::{DataSources, Query};
use crate::util::json_text;

#[derive(Clone, Copy)]
pub(super) enum Kind {
    /// First matching row (object) when criteria are given; every row (array) otherwise.
    One,
    Rows,
    Count,
}

pub(super) struct DataHelper {
    pub kind: Kind,
    pub sources: Arc<DataSources>,
}

fn query_of(a: &Args) -> Result<(String, Query), String> {
    let name = a.str(0).filter(|n| !n.trim().is_empty()).ok_or("the first argument is the data source name, e.g. 'users'")?;
    let rest = &a.params[1.min(a.params.len())..];
    if !rest.len().is_multiple_of(2) {
        return Err("criteria come in pairs: 'column' value".into());
    }
    let criteria = rest.chunks(2).map(|c| (json_text(&c[0]), json_text(&c[1]))).collect();
    let num = |k: &str| a.hash.get(k).and_then(|v| v.as_u64().or_else(|| json_text(v).trim().parse().ok())).map(|n| n as usize);
    Ok((name, Query { criteria, limit: num("limit"), offset: num("offset").unwrap_or(0) }))
}

impl DataHelper {
    fn compute(&self, a: &Args) -> Result<Value, String> {
        let (name, mut q) = query_of(a)?;
        match self.kind {
            Kind::One if !q.criteria.is_empty() => {
                q.limit = Some(1);
                Ok(self.sources.query(&name, &q)?.into_iter().next().map_or(Value::Null, Value::Object))
            }
            Kind::One | Kind::Rows => Ok(Value::Array(self.sources.query(&name, &q)?.into_iter().map(Value::Object).collect())),
            Kind::Count => {
                q.limit = None;
                q.offset = 0;
                Ok(json!(self.sources.query(&name, &q)?.len()))
            }
        }
    }
}

impl HelperDef for DataHelper {
    fn call_inner<'reg: 'rc, 'rc>(&self, h: &Helper<'rc>, _: &'reg Handlebars<'reg>, _: &'rc Context, _: &mut RenderContext<'reg, 'rc>) -> Result<ScopedJson<'rc>, RenderError> {
        let v = self.compute(&collect(h, None)).unwrap_or_else(|e| Value::String(error_text(&e)));
        Ok(ScopedJson::Derived(v))
    }

    fn call<'reg: 'rc, 'rc>(&self, h: &Helper<'rc>, _: &'reg Handlebars<'reg>, _: &'rc Context, _: &mut RenderContext<'reg, 'rc>, out: &mut dyn Output) -> HelperResult {
        let text = match self.compute(&collect(h, None)) {
            Ok(v) => render_value(&v, Mode::Json),
            Err(e) => error_text(&e),
        };
        out.write(&text)?;
        Ok(())
    }
}

pub(super) fn register(hb: &mut Handlebars<'static>, sources: Arc<DataSources>) {
    for (name, kind) in [("dataSource", Kind::One), ("dataRows", Kind::Rows), ("dataCount", Kind::Count)] {
        hb.register_helper(name, Box::new(DataHelper { kind, sources: sources.clone() }));
    }
}

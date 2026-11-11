use anyhow::Result;
use serde_json::Value;

pub trait SourceReader {
    fn read_documents(&self, inputs: &[String]) -> Result<Vec<Value>>;
}

pub trait TargetWriter {
    // For v0 we emit TypeScript seeds (string); later we can write JSON or direct inserts
    fn emit_seed(&self, docs: &[Value], out_dir: &str, options: &TargetOptions) -> Result<()>;
}

#[derive(Default, Clone)]
pub struct TargetOptions {
    pub collection: Option<String>,
    pub locale: Option<String>,
    pub related_collections: Option<Vec<String>>,
}

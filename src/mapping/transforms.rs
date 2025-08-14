use anyhow::{anyhow, Result};
use serde_json::{json, Value};

#[allow(dead_code)]
pub fn get_path<'a>(v: &'a Value, path: &str) -> Option<&'a Value> {
    let mut cur = v;
    for part in path.split('.') {
        if let Some(idx) = part.parse::<usize>().ok() {
            cur = cur.get(idx)?;
        } else {
            cur = cur.get(part)?;
        }
    }
    Some(cur)
}

#[allow(dead_code)]
pub fn to_point_from_latlng(lat: &Value, lng: &Value) -> Result<Value> {
    let lat = lat.as_f64().ok_or_else(|| anyhow!("lat not a number"))?;
    let lng = lng.as_f64().ok_or_else(|| anyhow!("lng not a number"))?;
    Ok(json!({"type":"Point","coordinates":[lng,lat]}))
}

// … add: split_comma, media_ref, maps for select, to_rich_text, etc.

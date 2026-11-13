//! `inspect`: what on the page can be acted on, and the selector that reaches it.
//!
//! A script is only as good as its selectors, and an agent pointed at an app it has never
//! seen has none. This answers the question it would otherwise answer by guessing: the
//! visible controls, each with its role, its accessible name, a CSS selector that matches it
//! and nothing else, and its box in CSS pixels, the same coordinates every mark carries.
//!
//! The list comes from the accessibility tree rather than a DOM walk. The tree has already
//! decided what a control is (a `<div role=button>`, a `<label>`-named input, a link styled
//! as a button) and what it is called, which is the name a person reading the page would use
//! and therefore the one an agent should reason with. The geometry comes from
//! `DOM.getBoxModel`, and the selector from the element itself.

use crate::cdp::Cdp;
use serde_json::{json, Value};

/// The roles a script can click, type into, hover or pick from. Anything else in the tree
/// (headings, paragraphs, generic containers) is context, not a target.
pub const ACTIONABLE: &[&str] = &[
    "button",
    "link",
    "textbox",
    "searchbox",
    "combobox",
    "checkbox",
    "radio",
    "switch",
    "tab",
    "menuitem",
    "menuitemcheckbox",
    "menuitemradio",
    "option",
    "slider",
    "spinbutton",
    "listbox",
    "treeitem",
    "PopUpButton",
    "ToggleButton",
];

/// Default and ceiling for how many elements one inspect returns. A page with a thousand
/// links is not served by a thousand-line answer; the caller can scroll and ask again.
pub const DEFAULT_LIMIT: usize = 150;
pub const MAX_LIMIT: usize = 1000;

/// Runs with `this` bound to the element. Returns the shortest selector it can find that
/// matches exactly one element, preferring the attributes a developer put there to be
/// stable (an id, a test id, a name, a label) over a structural path, which breaks the
/// moment the layout changes. Also reports whether something else is on top of the
/// element's centre, which is the same refusal `click` would make.
const SELECTOR_FN: &str = r#"function (cx, cy) {
  const el = this;
  if (!el || el.nodeType !== 1) return null;
  const uniq = (s) => { try { return document.querySelectorAll(s).length === 1; } catch (e) { return false; } };
  const q = (v) => '"' + String(v).replace(/["\\]/g, '\\$&') + '"';
  const tag = el.tagName.toLowerCase();
  let sel = null;
  if (el.id && uniq('#' + CSS.escape(el.id))) sel = '#' + CSS.escape(el.id);
  if (!sel) {
    for (const a of ['data-testid', 'data-test', 'data-test-id', 'data-cy', 'data-qa', 'name',
                     'aria-label', 'placeholder', 'title', 'for', 'href', 'type', 'value']) {
      const v = el.getAttribute(a);
      if (v && v.length < 120) {
        const s = tag + '[' + a + '=' + q(v) + ']';
        if (uniq(s)) { sel = s; break; }
      }
    }
  }
  if (!sel) {
    const parts = [];
    for (let n = el; n && n.nodeType === 1 && n !== document.documentElement; n = n.parentElement) {
      if (n !== el && n.id && uniq('#' + CSS.escape(n.id))) { parts.unshift('#' + CSS.escape(n.id)); }
      else {
        let p = n.tagName.toLowerCase();
        const par = n.parentElement;
        if (par) {
          const same = Array.from(par.children).filter((c) => c.tagName === n.tagName);
          if (same.length > 1) p += ':nth-of-type(' + (same.indexOf(n) + 1) + ')';
        }
        parts.unshift(p);
      }
      const s = parts.join(' > ');
      if (uniq(s)) { sel = s; break; }
      if (parts[0].charAt(0) === '#') break;
    }
    if (!sel) sel = parts.join(' > ');
  }
  let covered = null;
  if (cx >= 0 && cy >= 0 && cx < innerWidth && cy < innerHeight) {
    const t = document.elementFromPoint(cx, cy);
    if (t && t !== el && !el.contains(t) && !t.contains(el))
      covered = t.tagName.toLowerCase() + (t.id ? '#' + t.id : '');
  }
  const out = { selector: sel, tag: tag, covered: covered };
  if (el.disabled) out.disabled = true;
  if ('value' in el && typeof el.value === 'string' && el.value && tag !== 'button')
    out.value = el.value.length > 80 ? el.value.slice(0, 80) + '…' : el.value;
  return out;
}"#;

/// One quad from `DOM.getBoxModel` (eight numbers, four corners) as an axis-aligned box.
pub fn quad_box(q: &[f64]) -> Option<(f64, f64, f64, f64)> {
    if q.len() != 8 || q.iter().any(|v| !v.is_finite()) {
        return None;
    }
    let xs = [q[0], q[2], q[4], q[6]];
    let ys = [q[1], q[3], q[5], q[7]];
    let (x0, x1) = (
        xs.iter().cloned().fold(f64::INFINITY, f64::min),
        xs.iter().cloned().fold(f64::NEG_INFINITY, f64::max),
    );
    let (y0, y1) = (
        ys.iter().cloned().fold(f64::INFINITY, f64::min),
        ys.iter().cloned().fold(f64::NEG_INFINITY, f64::max),
    );
    Some((x0, y0, x1 - x0, y1 - y0))
}

/// Whether any of the box shows inside a `w` x `h` viewport.
pub fn in_view(b: (f64, f64, f64, f64), w: f64, h: f64) -> bool {
    b.2 > 0.0 && b.3 > 0.0 && b.0 + b.2 > 0.0 && b.1 + b.3 > 0.0 && b.0 < w && b.1 < h
}

/// The candidates in an `Accessibility.getFullAXTree` answer: not ignored, an actionable
/// role, and backed by a DOM node. As (backend node id, role, name), in tree order.
pub fn candidates(tree: &Value) -> Vec<(i64, String, String)> {
    let mut out = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for n in tree["nodes"]
        .as_array()
        .map(|a| a.as_slice())
        .unwrap_or(&[])
    {
        if n["ignored"].as_bool() == Some(true) {
            continue;
        }
        let role = n["role"]["value"].as_str().unwrap_or("");
        if !ACTIONABLE.contains(&role) {
            continue;
        }
        let Some(id) = n["backendDOMNodeId"].as_i64() else {
            continue;
        };
        // The tree can hold one DOM node twice (an inline text box and its owner).
        if !seen.insert(id) {
            continue;
        }
        let name = n["name"]["value"].as_str().unwrap_or("").trim().to_string();
        out.push((id, role.to_string(), name));
    }
    out
}

/// Run an inspect against the page. `op` carries the optional `limit` and `all` (include
/// elements scrolled out of view, which a script reaches because `click` scrolls first).
pub fn run(cdp: &mut Cdp, op: &Value, css_w: u32, css_h: u32) -> Result<Value, String> {
    let limit = match &op["limit"] {
        Value::Null => DEFAULT_LIMIT,
        v => v
            .as_u64()
            .filter(|n| *n >= 1)
            .map(|n| (n as usize).min(MAX_LIMIT))
            .ok_or_else(|| format!("inspect limit must be a whole number from 1, got {v}"))?,
    };
    let all = op["all"].as_bool().unwrap_or(false);
    let tree = cdp.send("Accessibility.getFullAXTree", json!({}))?;
    let found = candidates(&tree);
    let (vw, vh) = (css_w as f64, css_h as f64);
    let group = "kaviri-inspect";
    let mut elements = Vec::new();
    let mut truncated = false;
    let mut offscreen = 0usize;
    for (id, role, name) in found {
        if elements.len() >= limit {
            truncated = true;
            break;
        }
        // A node with no layout (display:none, a closed <details>) has no box model and is
        // not something a script can act on, so a failure here is a skip, not an error.
        let Ok(bm) = cdp.send("DOM.getBoxModel", json!({ "backendNodeId": id })) else {
            continue;
        };
        let quad: Vec<f64> = bm["model"]["border"]
            .as_array()
            .map(|a| a.iter().filter_map(|v| v.as_f64()).collect())
            .unwrap_or_default();
        let Some(b) = quad_box(&quad) else { continue };
        if b.2 < 1.0 || b.3 < 1.0 {
            continue;
        }
        let visible = in_view(b, vw, vh);
        if !visible {
            offscreen += 1;
            if !all {
                continue;
            }
        }
        let Ok(obj) = cdp.send(
            "DOM.resolveNode",
            json!({ "backendNodeId": id, "objectGroup": group }),
        ) else {
            continue;
        };
        let Some(oid) = obj["object"]["objectId"].as_str() else {
            continue;
        };
        let (cx, cy) = (b.0 + b.2 / 2.0, b.1 + b.3 / 2.0);
        let r = cdp.send(
            "Runtime.callFunctionOn",
            json!({
                "objectId": oid,
                "functionDeclaration": SELECTOR_FN,
                "arguments": [{ "value": cx }, { "value": cy }],
                "returnByValue": true,
            }),
        );
        let facts = match r {
            Ok(r) if r.get("exceptionDetails").is_none() => r["result"]["value"].clone(),
            _ => continue,
        };
        let Some(selector) = facts["selector"].as_str() else {
            continue;
        };
        let round = |v: f64| (v * 10.0).round() / 10.0;
        let mut e = json!({
            "role": role,
            "name": name,
            "selector": selector,
            "box": [round(b.0), round(b.1), round(b.2), round(b.3)],
            "in_view": visible,
        });
        for k in ["covered", "disabled", "value"] {
            if !facts[k].is_null() {
                e[k] = facts[k].clone();
            }
        }
        elements.push(e);
    }
    let _ = cdp.send(
        "Runtime.releaseObjectGroup",
        json!({ "objectGroup": group }),
    );
    let url = cdp
        .evaluate("location.href")
        .ok()
        .and_then(|v| v.as_str().map(String::from))
        .unwrap_or_default();
    let mut out = json!({
        "event": "inspect",
        "url": url,
        "viewport": [css_w, css_h],
        "count": elements.len(),
        "elements": elements,
    });
    if truncated {
        out["truncated"] = json!(true);
    }
    if !all && offscreen > 0 {
        out["offscreen"] = json!(offscreen);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_quad_becomes_its_bounding_box() {
        let q = [10.0, 20.0, 110.0, 20.0, 110.0, 60.0, 10.0, 60.0];
        assert_eq!(quad_box(&q), Some((10.0, 20.0, 100.0, 40.0)));
        assert_eq!(quad_box(&q[..6]), None);
        assert_eq!(quad_box(&[f64::NAN; 8]), None);
    }

    #[test]
    fn a_box_partly_on_screen_is_in_view_and_one_past_the_fold_is_not() {
        assert!(in_view((-50.0, 10.0, 100.0, 20.0), 800.0, 600.0));
        assert!(!in_view((10.0, 900.0, 100.0, 20.0), 800.0, 600.0));
        assert!(!in_view((10.0, 10.0, 0.0, 20.0), 800.0, 600.0));
    }

    #[test]
    fn only_actionable_unignored_dom_backed_nodes_are_candidates() {
        let tree = json!({"nodes": [
            {"role": {"value": "RootWebArea"}, "backendDOMNodeId": 1},
            {"role": {"value": "button"}, "name": {"value": " Save "}, "backendDOMNodeId": 7},
            {"role": {"value": "button"}, "name": {"value": "Save"}, "backendDOMNodeId": 7},
            {"role": {"value": "link"}, "ignored": true, "backendDOMNodeId": 8},
            {"role": {"value": "textbox"}, "name": {"value": "Email"}},
            {"role": {"value": "heading"}, "backendDOMNodeId": 9},
            {"role": {"value": "textbox"}, "name": {"value": "Name"}, "backendDOMNodeId": 10}
        ]});
        assert_eq!(
            candidates(&tree),
            vec![
                (7, "button".to_string(), "Save".to_string()),
                (10, "textbox".to_string(), "Name".to_string())
            ]
        );
    }
}

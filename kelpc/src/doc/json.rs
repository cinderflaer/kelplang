use super::*;

/// The `--format json` document: every module's declarations, members,
/// imports and trait impls, and the compiler's builtin members, as one object.
///
/// Everything in it is in a fixed order: modules by id, declarations and
/// imports in source order, builtins by key. Private items are included, with
/// `is_pub` saying which they are, because a tool that completes or navigates
/// needs every name. Spans are byte offsets into the file's text; lines are
/// 1-based. A member's `is_pub` is its own `pub` for a method, and its
/// declaration's for a field, a variant or a trait method.
pub fn json_document(modules: &[Module], builtins: &[crate::infer::BuiltinSig]) -> String {
    let mut sorted: Vec<&Module> = modules.iter().collect();
    sorted.sort_by(|a, b| a.id.cmp(&b.id));
    let mut o = String::from("{\"schema\":1,\"modules\":[");
    for (i, m) in sorted.iter().enumerate() {
        if i > 0 {
            o.push(',');
        }
        let scan = &m.scan;
        o.push_str(&format!(
            "{{\"id\":{},\"file\":{},\"doc\":{},\"items\":[",
            json_str(&m.id),
            json_str(&m.path),
            json_opt(scan.module_doc.as_deref()),
        ));
        for (k, item) in scan.items.iter().enumerate() {
            if k > 0 {
                o.push(',');
            }
            o.push_str(&format!(
                "{{\"kind\":{},\"name\":{},\"receiver\":{},\"name_span\":{},\"line\":{},\"end_line\":{},\"sig\":{},\"doc\":{},\"is_pub\":{},\"members\":{}}}",
                json_str(item.kind.word()),
                json_str(&item.name),
                json_opt(item.receiver.as_deref()),
                json_span(item.name_span),
                item.line,
                item.end_line,
                json_str(&item.sig),
                json_opt(item.doc.as_deref()),
                item.is_pub,
                json_members(&item.members),
            ));
        }
        o.push_str("],\"imports\":[");
        for (k, im) in scan.imports.iter().enumerate() {
            if k > 0 {
                o.push(',');
            }
            o.push_str(&format!(
                "{{\"path\":{},\"alias\":{},\"text\":{},\"line\":{},\"span\":{}}}",
                json_str(&im.path),
                json_str(&im.alias),
                json_str(&im.text),
                im.line,
                json_span(im.span),
            ));
        }
        o.push_str("],\"trait_impls\":[");
        for (k, imp) in scan.impls.iter().enumerate() {
            if k > 0 {
                o.push(',');
            }
            o.push_str(&format!(
                "{{\"trait\":{},\"target\":{},\"line\":{},\"end_line\":{},\"members\":{}}}",
                json_str(&imp.trait_name),
                json_str(&imp.ty),
                imp.line,
                imp.end_line,
                json_members(&imp.members),
            ));
        }
        o.push_str("]}");
    }
    o.push_str("],\"builtins\":[");
    for (k, b) in builtins.iter().enumerate() {
        if k > 0 {
            o.push(',');
        }
        let params: Vec<String> = b
            .params
            .iter()
            .map(|(name, ty)| format!("{{\"name\":{},\"ty\":{}}}", json_str(name), json_str(ty)))
            .collect();
        o.push_str(&format!(
            "{{\"key\":{},\"type\":{},\"name\":{},\"associated\":{},\"params\":[{}],\"ret\":{}}}",
            json_str(&b.key),
            json_str(&b.type_name),
            json_str(&b.name),
            b.associated,
            params.join(","),
            json_str(&b.ret),
        ));
    }
    o.push_str("]}\n");
    o
}

pub(super) fn json_members(members: &[Member]) -> String {
    let rows: Vec<String> = members
        .iter()
        .map(|m| {
            format!(
                "{{\"kind\":{},\"name\":{},\"name_span\":{},\"line\":{},\"end_line\":{},\"sig\":{},\"doc\":{},\"is_pub\":{}}}",
                json_str(m.kind.word()),
                json_str(&m.name),
                json_span(m.name_span),
                m.line,
                m.end_line,
                json_str(&m.sig),
                json_opt(m.doc.as_deref()),
                m.is_pub,
            )
        })
        .collect();
    format!("[{}]", rows.join(","))
}

pub(super) fn json_str(s: &str) -> String {
    crate::diag::json_string(s)
}

pub(super) fn json_opt(s: Option<&str>) -> String {
    match s {
        Some(s) => json_str(s),
        None => "null".to_string(),
    }
}

pub(super) fn json_span((start, end): (usize, usize)) -> String {
    format!("[{start},{end}]")
}

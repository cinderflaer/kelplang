//! Inline assembly template text: the operand numbering the LLVM backend writes
//! into an `asm(...)` block's template before LLVM sees it.

/// A template with each `{k}` placeholder replaced by `sigil` and
/// `numbers[k]`, the backend's own operand number. A literal `sigil` is
/// doubled, which is how LLVM reads `$$` in its inline assembly templates.
pub(crate) fn asm_operand_text(template: &str, numbers: &[usize], sigil: char) -> String {
    let chars: Vec<char> = template.chars().collect();
    let mut out = String::with_capacity(template.len());
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c == sigil {
            out.push(sigil);
            out.push(sigil);
            i += 1;
        } else if c == '{' {
            let close = chars[i + 1..].iter().position(|&c| c == '}');
            let number = close.and_then(|close| {
                chars[i + 1..i + 1 + close]
                    .iter()
                    .collect::<String>()
                    .parse::<usize>()
                    .ok()
                    .map(|n| (n, close))
            });
            match number {
                Some((n, close)) if n < numbers.len() => {
                    out.push(sigil);
                    out.push_str(&numbers[n].to_string());
                    i += close + 2;
                }
                _ => {
                    out.push(c);
                    i += 1;
                }
            }
        } else {
            out.push(c);
            i += 1;
        }
    }
    out
}

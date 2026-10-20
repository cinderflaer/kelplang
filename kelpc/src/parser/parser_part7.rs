use super::*;

impl Parser {
    /// Splits column `c`, where the first row writes a nested tuple pattern,
    /// into one column per element (`cols[c].f0`, `cols[c].f1`, ...). A row that
    /// writes a tuple there contributes its elements; a row that writes a name
    /// or `_` matches every element and binds the name to the whole position.
    pub(crate) fn expand_tuple_column(&mut self, rows: Vec<TupleRow>, cols: &[Expr], c: usize, span: Span) -> PResult<Expr> {
        let TuplePat::Tuple(first_elems, _) = &rows[0].pats[c] else { unreachable!("the caller found a tuple here") };
        let width = first_elems.len();
        let mut new_cols: Vec<Expr> = cols[..c].to_vec();
        for j in 0..width {
            new_cols.push(Expr::FieldAccess {
                base: Box::new(cols[c].clone()),
                field: crate::types::tuple_field_name(j),
                span,
            });
        }
        new_cols.extend(cols[c + 1..].iter().cloned());
        let mut next_rows = Vec::with_capacity(rows.len());
        for row in rows {
            let mut pats: Vec<TuplePat> = row.pats[..c].to_vec();
            let mut binds = row.binds.clone();
            match &row.pats[c] {
                TuplePat::Tuple(elems, tspan) => {
                    if elems.len() != width {
                        return Err(ParseError {
                            help: None,
                            message: format!("this nested tuple pattern has {} positions and an earlier arm's has {width}", elems.len()),
                            span: *tspan,
                        });
                    }
                    pats.extend(elems.iter().cloned());
                }
                TuplePat::Any(name) => {
                    if let Some(n) = name {
                        binds.push((n.clone(), cols[c].clone()));
                    }
                    pats.extend(std::iter::repeat(TuplePat::Any(None)).take(width));
                }
                TuplePat::Variant { span: vspan, .. } => {
                    return Err(ParseError {
                        help: None,
                        message: "a position that is a tuple in one arm cannot be a variant in another".to_string(),
                        span: *vspan,
                    })
                }
                TuplePat::Lit { expr, .. } => {
                    return Err(ParseError {
                        help: None,
                        message: "a position that is a tuple in one arm cannot be a literal in another".to_string(),
                        span: expr.span(),
                    })
                }
            }
            pats.extend(row.pats[c + 1..].iter().cloned());
            next_rows.push(TupleRow { pats, binds, body: row.body, span: row.span });
        }
        self.compile_tuple_rows(next_rows, &new_cols, span)
    }

    /// Column `c`, where the first row writes a literal: `if col == lit { ... }
    /// else if col == other { ... } else { rows that match anything }`. A column
    /// of literals needs an arm that matches anything there, since a number has
    /// no list of values to be exhaustive over.
    pub(crate) fn compile_literal_column(&mut self, rows: Vec<TupleRow>, cols: &[Expr], c: usize, span: Span) -> PResult<Expr> {
        let mut lits: Vec<(String, Expr)> = Vec::new();
        for row in &rows {
            match &row.pats[c] {
                TuplePat::Lit { key, expr } => {
                    if !lits.iter().any(|(k, _)| k == key) {
                        lits.push((key.clone(), expr.clone()));
                    }
                }
                TuplePat::Any(_) => {}
                TuplePat::Variant { span: vspan, .. } => {
                    return Err(ParseError {
                        help: None,
                        message: "a position cannot be matched by a literal in one arm and a variant in another".to_string(),
                        span: *vspan,
                    })
                }
                TuplePat::Tuple(_, tspan) => {
                    return Err(ParseError {
                        help: None,
                        message: "a position cannot be matched by a literal in one arm and a tuple in another".to_string(),
                        span: *tspan,
                    })
                }
            }
        }
        let any_rows = |this: &mut Parser, rows: &[TupleRow], want: Option<&str>| -> Vec<TupleRow> {
            let _ = this;
            let mut group = Vec::new();
            for row in rows {
                let mut next = row.clone();
                match &row.pats[c] {
                    TuplePat::Lit { key, .. } if Some(key.as_str()) == want => {}
                    TuplePat::Lit { .. } => continue,
                    TuplePat::Any(name) => {
                        if let Some(n) = name {
                            next.binds.push((n.clone(), cols[c].clone()));
                        }
                    }
                    _ => continue,
                }
                next.pats[c] = TuplePat::Any(None);
                group.push(next);
            }
            group
        };
        let fallback = any_rows(self, &rows, None);
        if fallback.is_empty() {
            return Err(ParseError {
                help: None,
                message: format!("{NO_LITERAL_FALLBACK}: add an arm with a name or '_' in that position, or 'else'"),
                span: rows[0].span,
            });
        }
        let mut acc: ElseBranch = {
            let body = self.compile_tuple_rows(fallback, cols, span)?;
            ElseBranch::Block(Block { stmts: vec![Stmt::Expr { expr: body, span }], span })
        };
        for (key, lit) in lits.into_iter().rev() {
            let group = any_rows(self, &rows, Some(&key));
            let body = self.compile_tuple_rows(group, cols, span)?;
            let cond = Expr::Binary {
                op: BinOp::Eq,
                lhs: Box::new(cols[c].clone()),
                rhs: Box::new(lit),
                span,
            };
            acc = ElseBranch::If(Expr::If {
                cond: Box::new(cond),
                then_block: Block { stmts: vec![Stmt::Expr { expr: body, span }], span },
                else_block: Some(Box::new(acc)),
                span,
            });
        }
        match acc {
            ElseBranch::If(e) => Ok(e),
            ElseBranch::Block(b) => Ok(Expr::Block(b)),
        }
    }
}

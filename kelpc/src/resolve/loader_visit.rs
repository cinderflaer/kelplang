use super::*;

impl Loader<'_> {
    /// Registers the manifest's `[alias]` entries, discovers the module graph,
    /// then loads it for real.
    ///
    /// Two phases, because the two halves of M5.3 need each other: an `impl`
    /// path may contain `_alias`, so the file graph is only knowable after
    /// expansion, while the alias table is only complete once every file is
    /// known. Discovery therefore tolerates what it cannot yet resolve - a
    /// file that fails to expand or parse is skipped, an unresolvable import
    /// is ignored - and just collects aliases; the final strict round runs
    /// against the complete table, where every failure is a real error.
    pub(crate) fn load_project(&mut self) -> Result<(), Diagnostic> {
        self.load_manifest_aliases()?;

        let mut last_error = None;
        for _ in 0..MAX_ALIAS_ROUNDS {
            // Discovery: close over every file reachable without the aliases
            // this round has not collected yet.
            loop {
                let before = self.raw.len();
                self.reset_round();
                let entry = self.entry.clone();
                // Discovery never fails: it skips what it cannot resolve yet.
                let _ = self.visit(&entry, Span::new(0, 0), "", false);
                if self.raw.len() == before {
                    break;
                }
            }

            let before = self.raw.len();
            self.reset_round();
            let entry = self.entry.clone();
            let result = self.visit(&entry, Span::new(0, 0), "", true);
            if self.raw.len() == before {
                // Nothing new was loaded, so no later round could resolve
                // more aliases: this result is final.
                result?;
                // Catches a cycle nothing references; expansion only finds
                // one it actually expands.
                return self.aliases.check_cycles();
            }
            // The strict round found a file via an aliased path, which may in
            // turn have contributed the alias an earlier error was about.
            last_error = result.err();
        }

        Err(last_error.unwrap_or_else(|| {
            Diagnostic::new(
                format!("alias expansion did not settle after {MAX_ALIAS_ROUNDS} rounds"),
                Span::new(0, 0),
                "",
            )
        }))
    }

    /// A round is one full load attempt: the discovery pass, then the strict
    /// pass that decides the round. Everything a round produced is discarded
    /// here so that the last round is the only one anything reads - including
    /// the deprecations, which both passes collect. Leaving them in printed
    /// every outdated finding twice, once for the discovery pass and once for
    /// the strict one.
    pub(crate) fn reset_round(&mut self) {
        self.state.clear();
        self.stack.clear();
        self.modules.clear();
        self.prefixes.clear();
        self.parsed.clear();
        self.deprecations.clear();
    }

    /// Defines the manifest's `[alias]` entries. They go in before any inline
    /// declaration is scanned, so a module that re-declares one of their names
    /// is the duplicate that gets reported, naming the manifest.
    pub(crate) fn load_manifest_aliases(&mut self) -> Result<(), Diagnostic> {
        if let Some((path, text)) = self.manifest_alias_source.take() {
            self.sources.insert(preprocess::MANIFEST_MODULE, &path, text);
        }
        for def in std::mem::take(&mut self.manifest_alias) {
            self.aliases.define(def)?;
        }
        Ok(())
    }

    /// `strict == false` is the discovery round: every failure is skipped, so
    /// a file loaded later can still contribute the alias an earlier file
    /// used.
    pub(crate) fn visit(&mut self, id: &str, at: Span, from: &str, strict: bool) -> Result<(), Diagnostic> {
        let depth = self.stack.len();
        let result = self.visit_body(id, at, from, strict);
        if result.is_err() {
            // A failed visit must not leave the module looking half-loaded -
            // a sibling import should be free to try it again.
            self.stack.truncate(depth);
            self.state.remove(id);
        }
        result
    }

    /// The raw token stream for one module, loaded from disk on first sight.
    /// Inline `alias` declarations are collected here, from the *unexpanded*
    /// tokens, so every file the walk reaches contributes its aliases.
    pub(crate) fn tokens_of(&mut self, id: &str, at: Span, from: &str) -> Result<Vec<Token>, Diagnostic> {
        if let Some(tokens) = self.raw.get(id) {
            return Ok(tokens.clone());
        }
        // A `std/...` id comes from the compiler's own shipped sources (M6.2)
        // instead of the project directory; everything downstream - lexing,
        // aliases, parsing, prefixing - is identical.
        let (path, text) = match crate::stdlib::source_configured(id, self.http_pool) {
            Some(text) => (self.src_dir.join(id).with_extension(EXT), text),
            None => {
                let path = self.src_dir.join(format!("{id}.{EXT}"));
                // `kelpc check --text` hands the entry module's buffer in on
                // stdin (MISSING-b6Ytr): the path still names the file on
                // disk, an editor's unsaved edit does not, so an unsaved
                // buffer gets a real diagnostic instead of the last save's.
                let text = if id == self.entry {
                    match self.entry_text.take() {
                        Some(text) => text,
                        None => fs::read_to_string(&path).map_err(|_| {
                            Diagnostic::new(
                                format!("cannot find module '{id}': expected '{}'", path.display()),
                                at,
                                from,
                            )
                            .with_help("create the file at that path, under src/, or correct the import path.")
                        })?,
                    }
                } else {
                    fs::read_to_string(&path).map_err(|_| {
                        Diagnostic::new(
                            format!("cannot find module '{id}': expected '{}'", path.display()),
                            at,
                            from,
                        )
                        .with_help("create the file at that path, under src/, or correct the import path.")
                    })?
                };
                (path, text)
            }
        };
        self.sources.insert(id, &path, text.clone());

        let tokens = Lexer::new(&text)
            .tokenize()
            .map_err(|e| Diagnostic::new(e.message, e.span, id))?;
        for def in preprocess::scan_inline_aliases(&tokens, &text, id)? {
            self.aliases.define(def)?;
        }
        self.raw.insert(id.to_string(), tokens.clone());
        Ok(tokens)
    }

    pub(crate) fn visit_body(&mut self, id: &str, at: Span, from: &str, strict: bool) -> Result<(), Diagnostic> {
        match self.state.get(id) {
            Some(2) => return Ok(()),
            Some(_) => {
                let start = self.stack.iter().position(|m| m == id).unwrap_or(0);
                let mut chain: Vec<String> = self.stack[start..].to_vec();
                chain.push(id.to_string());
                return Err(Diagnostic::new(
                    format!("import cycle: {}", chain.join(" -> ")),
                    at,
                    from,
                ));
            }
            _ => {}
        }

        let tokens = self.tokens_of(id, at, from)?;

        if !strict {
            // Discovery. Imports come straight off the raw token stream, so a
            // file whose body cannot expand yet - it uses an alias another
            // file has still to contribute - nevertheless contributes its own
            // aliases and its unaliased imports. Unresolvable paths are
            // skipped here and become real errors in the strict round.
            self.state.insert(id.to_string(), 1);
            self.stack.push(id.to_string());
            for (path, span) in raw_imports(&tokens) {
                if validate_impl_path(&path, span, id).is_ok() {
                    let _ = self.visit(&path, span, id, false);
                }
            }
            self.stack.pop();
            self.state.insert(id.to_string(), 2);
            return Ok(());
        }

        let expanded = self.aliases.expand_file(&tokens, id)?;
        let mut program = Parser::new(expanded)
            .parse_program()
            .map_err(|e| {
                let mut diag = Diagnostic::new(e.message, e.span, id);
                diag.help = e.help;
                diag
            })?;
        // `defer` is lowered away here, before resolution, the type checker,
        // the borrow checker and both backends, so every deferred action is
        // ordinary code by the time any of them sees the tree
        // (`kelp wiki page language/statics-and-defer.md`).
        crate::defer::lower(&mut program, id)?;

        self.state.insert(id.to_string(), 1);
        self.stack.push(id.to_string());

        let mut aliases = HashMap::new();
        let mut seen_aliases: HashSet<String> = HashSet::new();
        for imp in &program.impls {
            validate_impl_path(&imp.path, imp.span, id)?;
            let alias = default_alias(&imp.path);
            validate_alias(&alias, imp.span, id)?;
            if !seen_aliases.insert(alias.clone()) {
                return Err(Diagnostic::new(
                    format!("alias '{alias}' is already used by another import in this module"),
                    imp.span,
                    id,
                ));
            }
            for grant in &imp.caps {
                if !crate::stdlib::is_domain(&grant.domain) {
                    return Err(Diagnostic::new(
                        format!(
                            "'{}' is not a capability domain - v1 has {}",
                            grant.domain,
                            crate::stdlib::DOMAINS.join(", ")
                        ),
                        grant.span,
                        id,
                    ));
                }
                validate_cap_grant(&grant.domain, &grant.patterns, grant.span, id)?;
                self.grants.push(GrantEdge {
                    importer: id.to_string(),
                    target: imp.path.clone(),
                    domain: grant.domain.clone(),
                    patterns: grant.patterns.clone(),
                    span: grant.span,
                });
            }
            if imp.caps_all {
                self.all_grants.push(AllGrantEdge {
                    importer: id.to_string(),
                    target: imp.path.clone(),
                    span: imp.span,
                });
            }
            let target = imp.path.clone();
            aliases.insert(alias, target.clone());
            self.visit(&target, imp.span, id, strict)?;
            // MK7 §8: importing a module that holds a capability domain is what
            // makes generated code call the capability matcher, and
            // `std/path`'s own `normalize` calls the normalizer; both live in
            // Kelp (`std/rt`) rather than in C text emitted into the program.
            // Injecting it at the import that needs it - keyed on the same
            // conditions as the calls themselves - is what keeps the runtime out
            // of programs that use neither, and what makes the module the
            // emitted C calls into always present.
            if domain_of_with_extern(&target, &self.extern_modules).is_some() || target == "std/path" {
                self.visit(crate::stdlib::RT_MODULE, imp.span, id, strict)?;
            }
        }

        self.stack.pop();
        self.state.insert(id.to_string(), 2);

        // Dependencies are pushed first (post-order), so by the time a
        // module is pushed here every prefix it needs is already known.
        let prefix = if id == self.entry {
            String::new()
        } else {
            crate::stdlib::key_prefix(id)
        };
        self.prefixes.insert(id.to_string(), prefix.clone());
        self.modules.push(ModuleInfo {
            id: id.to_string(),
            prefix,
            aliases,
        });
        self.parsed.insert(id.to_string(), program);
        Ok(())
    }
}

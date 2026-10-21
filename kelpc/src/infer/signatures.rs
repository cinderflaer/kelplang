use super::*;

impl<'a> SignatureScope<'a> {
    pub(crate) fn new(
        project: &'a Project,
        module: &'a str,
        item: &'a str,
        fix: &'a str,
        type_params: &'a [String],
        exported: &'a std::collections::HashSet<String>,
        declared: &'a std::collections::HashSet<&'a str>,
    ) -> SignatureScope<'a> {
        SignatureScope {
            project,
            module,
            prefix: project.prefix_of(module),
            aliases: project.alias_prefixes_of(module),
            item,
            fix,
            type_params: type_params.iter().map(String::as_str).collect(),
            exported,
            declared,
        }
    }

    pub(crate) fn check_fields(&self, fields: &[ast::FieldDecl]) -> Result<()> {
        for f in fields {
            self.check_type(&f.ty)?;
        }
        Ok(())
    }

    pub(crate) fn check_function(&self, f: &ast::FunctionDecl) -> Result<()> {
        for p in &f.params {
            self.check_type(&p.ty)?;
        }
        if let Some(ret) = &f.return_type {
            self.check_type(ret)?;
        }
        Ok(())
    }

    /// One written type, walked to the bottom: a container's element, a
    /// `Map`'s key and value, an array's element (`[T; N]`), a generic
    /// instantiation's arguments, a function type's parameters and return,
    /// and a borrow's pointee. A name is refused only when it is *declared*
    /// and its module does not export it, so an unknown name and a builtin
    /// both fall through to the messages that already report them.
    pub(crate) fn check_type(&self, ty: &ast::TypeExpr) -> Result<()> {
        if let Some(elem) = &ty.array_elem {
            return self.check_type(elem);
        }
        if let Some(elems) = &ty.tuple {
            for e in elems {
                self.check_type(e)?;
            }
            return Ok(());
        }
        if let Some(fn_params) = &ty.fn_params {
            for p in fn_params {
                self.check_type(&p.ty)?;
            }
            if let Some(ret) = &ty.fn_ret {
                self.check_type(ret)?;
            }
            return Ok(());
        }
        for arg in &ty.args {
            self.check_type(arg)?;
        }
        // A type parameter is not a declared type, and it shadows a name of
        // its own while an instantiation is resolved (A-2).
        if ty.args.is_empty()
            && !ty.name.contains('.')
            && self.type_params.contains(&ty.name.as_str())
        {
            return Ok(());
        }
        if Ty::from_name(&ty.name).is_some()
            || matches!(ty.name.as_str(), "List" | "Box" | "Map" | "Atomic" | "Thread" | "Mutex" | "RwLock")
        {
            return Ok(());
        }
        let key = qualify_written(&ty.name, self.prefix, &self.aliases);
        if self.declared.contains(key.as_str()) && !self.exported.contains(&key) {
            let bare = ty.name.rsplit('.').next().unwrap_or(&ty.name);
            let owner = self.type_module(&ty.name);
            return Err(TypeError {
                help: None,
                message: format!(
                    "'{}' is exported but its signature names '{key}', which module '{owner}' does not export: add 'pub' to '{bare}', or make '{}' private",
                    self.item, self.fix
                ),
                span: ty.span,
            });
        }
        Ok(())
    }

    /// The module a written type name belongs to: the declaring module for a
    /// bare name, the alias target for an `alias.Item` spelling.
    pub(crate) fn type_module(&self, name: &str) -> &str {
        let Some((alias, _)) = name.split_once('.') else {
            return self.module;
        };
        self.project
            .modules
            .iter()
            .find(|m| m.id == self.module)
            .and_then(|m| m.aliases.get(alias))
            .map(String::as_str)
            .unwrap_or(self.module)
    }
}

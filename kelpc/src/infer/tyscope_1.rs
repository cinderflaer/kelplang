use super::*;

impl TypeScope<'_> {
    /// Whether `ty` is an `enum` (declared or instantiated), as `Ctx::is_enum_ty` says.
    pub(crate) fn is_enum_ty(&self, ty: &Ty) -> bool {
        matches!(ty, Ty::Struct(name) if self.enums.contains_key(name) || self.env.instances.borrow().sums.contains_key(name))
    }

    /// The user types that are `Map` keys, from the declarations this view of
    /// the program can see (A-6, A-12). A `Map` key is resolved before the
    /// whole program is known, so this runs the same fixpoint `Ctx` runs, over
    /// the declarations gathered so far: a type a field could legally name is
    /// always an earlier declaration, and an earlier declaration is present.
    pub(crate) fn keyable_set(&self) -> std::collections::HashSet<String> {
        let decls = type_decls_of(self.structs, self.enums, &self.env.instances.borrow());
        crate::types::map_key_types(&decls, &self.env.declared_keys, &self.env.declared_drops)
    }

    /// A written type name, or a container over one. `indirect` is true
    /// when the type is being resolved *inside* a `Box`/`List` argument,
    /// where referring to the `datac` currently being declared (or to a
    /// `datac` declared later) is legal: both containers hold a pointer, so
    /// the indirection is what makes a recursive type finite.
    pub(crate) fn resolve(&self, ty: &ast::TypeExpr, indirect: bool) -> Result<Ty> {
        let resolved = self.resolve_base(ty, indirect)?;
        // A `datac` that contains itself *by value* has no finite size.
        // Through a container it is fine - that is the whole point of `Box`
        // and of a list's pointer buffer - so the check is skipped there.
        if !indirect {
            if let Some(ip) = self.in_progress {
                if resolved == Ty::Struct(ip.to_string()) {
                    return Err(TypeError {
                        help: None,
                        message: format!(
                            "'{ip}' cannot hold itself by value - that type has no finite size; put it behind indirection ('val next: Box<{ip}>', or a 'List<{ip}>' for many)"
                        ),
                        span: ty.span,
                    });
                }
            }
        }
        Ok(resolved)
    }

    /// Whether a canonical key names a declaration of the module this scope
    /// belongs to. A canonical key carries its declaring module's symbol
    /// prefix (`shapes.circle.Circle`), which is how a receiver declaration
    /// arrives here, and a module's own keys are that prefix plus one bare
    /// identifier. The entry module's prefix is empty and its keys are the
    /// unprefixed names, so a key that keeps a dot in it is never one of its
    /// own.
    pub(crate) fn owns_canonical_key(&self, key: &str) -> bool {
        key.strip_prefix(self.prefix)
            .is_some_and(|item| !item.contains('.'))
    }

    pub(crate) fn resolve_base(&self, ty: &ast::TypeExpr, indirect: bool) -> Result<Ty> {
        // A const argument (`Buf<16>`, A-20) only means something where a
        // `const` parameter's declaration says so; the generic-instantiation
        // path folds it there. Anywhere else it is a value written where a
        // type belongs.
        if ty.const_expr.is_some() {
            return Err(TypeError {
                help: None,
                message: format!(
                    "'{}' is a constant expression, not a type: a constant argument is only legal where the declaration's parameter is 'const' ('datac Buf<const N: usize> ...'), and a type belongs here",
                    ty.display()
                ),
                span: ty.span,
            });
        }
        // `impl Trait` in argument position (A-20) is desugared at the
        // declaration, so any one still here is in a position the sugar does
        // not cover (a `let`, a field, a return).
        if let Some(trait_name) = &ty.impl_trait {
            return Err(TypeError {
                help: None,
                message: format!(
                    "'impl {trait_name}' is only legal as a whole parameter type of a declared function: write 'fun f(x: impl {trait_name})', and name a type parameter where a stored, returned or container type belongs"
                ),
                span: ty.span,
            });
        }
        // `scoped` is a *parameter* modifier (`kelp wiki page language/functions.md` part 2)
        // and is handled by `resolve_param_mode`, before this is reached. Any
        // other position - a return type, a `val` annotation, a `datac`
        // field, another function type's own parameter - would store the
        // value, which is exactly what the modifier forbids.
        if ty.scoped {
            return Err(TypeError {
                help: None,
                message: format!(
                    "'scoped {}' is only legal on a top-level parameter of a declaration, not here: a scoped function value may only live for the call it was written in, so it cannot be stored, returned or nested in another type",
                    ty.display()
                ),
                span: ty.span,
            });
        }
        // A borrowed trait object (A-12 slice 4): `&dyn Name` / `&mut dyn
        // Name`. The trait must be declared and object safe; the legal
        // positions (a parameter or a local) are enforced by the callers that
        // would otherwise store the value, exactly as they are for `Ty::Ref`
        // (`is_borrowed_field_ty` and `resolve_element`).
        if let Some(written) = &ty.dyn_trait {
            // A bare `dyn Name` is legal only as the argument of a `Box`, which
            // `resolve_base` handles in its `Box` arm before the argument is
            // resolved, so reaching here without the `&` is the bare spelling
            // everywhere else (A-12 slice 7).
            if !ty.amp {
                return Err(TypeError {
                    help: None,
                    message: format!(
                        "'dyn {written}' is not a type on its own: an owned trait object is written 'Box<dyn {written}>', and a borrowed one '&dyn {written}'"
                    ),
                    span: ty.span,
                });
            }
            let qualified = self.qualify(written, ty.span)?;
            if !self.env.traits.contains_key(&qualified) {
                return Err(TypeError {
                    help: None,
                    message: format!(
                        "'&dyn {written}' names no declared trait: declare 'trait {written} {{ ... }}' first (or import the module that does)"
                    ),
                    span: ty.span,
                });
            }
            // A trait named through another module's alias has to be exported
            // by that module, exactly as a bound on the same trait does.
            if let Some((alias, item)) = written.split_once('.') {
                if !exported(&qualified, &self.env) {
                    return Err(private_item_error(alias, item, ty.span));
                }
            }
            let spelling = if ty.amp_mut {
                format!("&mut dyn {written}")
            } else {
                format!("&dyn {written}")
            };
            check_dyn_object_safety(&spelling, &qualified, ty.span, &self.env.traits)?;
            self.env.dyn_traits.borrow_mut().insert(qualified.clone());
            return Ok(Ty::Dyn {
                mutable: ty.amp_mut,
                trait_name: qualified,
            });
        }
        // A tuple type (`design/syntax.md (kelp wiki)` 3): `(A, B)`. Every element is stored
        // by value, exactly as a `datac` field is, so the element rules are the
        // field rules: an element may not be a borrow, and `unit` cannot be
        // stored either. `indirect` is passed through, because a tuple inside a
        // `Box`/`List` argument is behind a pointer just as a named type is
        // there, so an element declared later is legal in that position.
        if let Some(elems) = &ty.tuple {
            let mut resolved = Vec::with_capacity(elems.len());
            for (i, e) in elems.iter().enumerate() {
                if e.amp {
                    return Err(TypeError {
                        help: None,
                        message: format!(
                            "'&{}' cannot be element {} of a tuple: an element is stored by value, so a borrow of one has nowhere to put its lifetime; use the owned type",
                            e.display(),
                            i + 1
                        ),
                        span: e.span,
                    });
                }
                let elem = self.resolve(e, indirect)?;
                if crate::types::is_borrowed_field_ty(&elem) {
                    return Err(TypeError {
                        help: None,
                        message: format!(
                            "element {} of the tuple is a borrowed type ('{}'): a tuple holds owned data only in v1, exactly as a datac field does, so a '&str', 'View' or reference element is not expressible; an owned 'String' is the way to keep text in one",
                            i + 1,
                            elem.display()
                        ),
                        span: e.span,
                    });
                }
                if elem == Ty::Unit {
                    return Err(TypeError {
                        help: None,
                        message: format!(
                            "element {} of the tuple is 'unit': 'unit' is the absent value and cannot be stored, exactly as a datac field cannot be",
                            i + 1
                        ),
                        span: e.span,
                    });
                }
                resolved.push(elem);
            }
            return Ok(tuple_ty(&self.env, resolved));
        }
        // A fixed-size array (A-18): `[T; N]`. The length is a compile-time
        // constant expression (A-19) folded here, once, so the type is a
        // concrete `[T; N]` for every pass after this one.
        if let Some(elem) = &ty.array_elem {
            let len_expr = ty
                .array_len
                .as_ref()
                .expect("the parser builds an array type with both halves");
            let len = self.array_len(len_expr)?;
            let elem = self.resolve_element(elem)?;
            if !crate::types::is_array_element(&elem) {
                return Err(TypeError {
                    help: None,
                    message: array_element_error(&elem),
                    span: ty.span,
                });
            }
            note_heap_use(&self.env, managed::HeapSlot::ArrayElement, &elem, ty.span);
            return Ok(Ty::Array(Box::new(elem), len));
        }
        // A function type (A-4): `fun(i32, String) => bool`. Checked before
        // every name, because it is the one written type that is not one -
        // and `fun` cannot be a name at all, since it is a keyword. A closure
        // type (A-4 phase E) is the same shape under `closure`.
        if let Some(params) = &ty.fn_params {
            if ty.is_closure() {
                return self.resolve_closure_type(ty, params);
            }
            return self.resolve_fn_type(ty, params);
        }
        // `I::Item` / `Self::Item` (A-12 slice 5): an associated type projected
        // from a type parameter, or from `Self` in a trait signature. The
        // projection's own key is stored beside the parameter's binding, so a
        // bound that declares no such type is refused here.
        if let Some(item) = &ty.assoc {
            let key = format!("{}::{}", ty.name, item);
            if let Some(bound) = self.params.get(&key) {
                return Ok(bound.clone());
            }
            return Err(TypeError {
                help: None,
                message: format!(
                    "'{}::{}' is not a known associated type: bind '{}' with a trait that carries 'type {}' ('trait ... {{ type {} }}'), so the projection names the type an impl supplies",
                    ty.name, item, ty.name, item, item
                ),
                span: ty.span,
            });
        }
        // A type parameter in scope (`datac Pair<A, B>`'s `A`) is whatever
        // the instantiation bound it to (A-2). Checked before every other
        // name, so a parameter shadows a type of the same name.
        if ty.args.is_empty() && !ty.name.contains('.') {
            if let Some(bound) = self.params.get(&ty.name) {
                if let Ty::ConstInt { .. } = bound {
                    return Err(TypeError {
                        help: None,
                        message: format!(
                            "'{}' is a const parameter, not a type: use it as a value ('i < {}'), as an array length ('[i32; {}]') or in a repeat expression ('[0; {}]'), and write a type parameter where a type belongs",
                            ty.name, ty.name, ty.name, ty.name
                        ),
                        span: ty.span,
                    });
                }
                return Ok(bound.clone());
            }
        }
        // The generic names v1 knows (`List<T>` from C1, `Box<T>` from M8,
        // `Map<V>` from MK5-S5, `Atomic<T>` from MK9-S3-M1 stage A1, `Thread<T>`
        // from its stage B1). All are compiler builtins, not user-definable
        // types; each checks its element against what its representation can
        // hold.
        // `Box<dyn Name>` (A-12 slice 7): the one place a bare `dyn Name` is a
        // type. It is handled before the element checks below, because a trait
        // object is not an element type: the box owns the erased value itself,
        // not a value of the element's type. The trait must be declared and
        // object safe, exactly as it must be behind a `&dyn`, and the resolved
        // type name is recorded so the pair's C typedef exists even when
        // nothing coerces to it.
        if ty.name == "Box" && ty.args.len() == 1 && ty.args[0].dyn_trait.is_some() {
            let arg = &ty.args[0];
            let written = arg.dyn_trait.as_ref().expect("matched above");
            if arg.amp {
                return Err(TypeError {
                    help: None,
                    message: format!(
                        "'Box<&dyn {written}>' is not an owned trait object: a box holds its value, so write 'Box<dyn {written}>' for an owned one or '&dyn {written}' for a borrowed one"
                    ),
                    span: arg.span,
                });
            }
            let qualified = self.qualify(written, arg.span)?;
            if !self.env.traits.contains_key(&qualified) {
                return Err(TypeError {
                    help: None,
                    message: format!(
                        "'Box<dyn {written}>' names no declared trait: declare 'trait {written} {{ ... }}' first (or import the module that does)"
                    ),
                    span: arg.span,
                });
            }
            if let Some((alias, item)) = written.split_once('.') {
                if !exported(&qualified, &self.env) {
                    return Err(private_item_error(alias, item, arg.span));
                }
            }
            check_dyn_object_safety(
                &format!("Box<dyn {written}>"),
                &qualified,
                arg.span,
                &self.env.traits,
            )?;
            self.env.dyn_traits.borrow_mut().insert(qualified.clone());
            return Ok(Ty::BoxDyn {
                trait_name: qualified,
            });
        }
        if ty.name == "List"
            || ty.name == "Box"
            || ty.name == "Map"
            || ty.name == "Atomic"
            || ty.name == "Thread"
            || ty.name == "Mutex"
            || ty.name == "RwLock"
            || ty.name == "Ptr"
        {
            // `Map<K, V>` names its key type, and `Map<V>` is the shorthand
            // for a `String` key - which is what every use site written before
            // A-2 says, and the two spellings are the same `Ty`.
            let (key, value_arg, value_span) = if ty.name == "Map" {
                match ty.args.len() {
                    1 => (Ty::String, &ty.args[0], ty.args[0].span),
                    2 => {
                        let key = self.resolve_element(&ty.args[0])?;
                        if !crate::types::is_map_key_with(&key, &self.keyable_set()) {
                            return Err(TypeError {
                                help: None,
                                message: map_key_error(&key),
                                span: ty.args[0].span,
                            });
                        }
                        note_heap_use(&self.env, managed::HeapSlot::MapKey, &key, ty.args[0].span);
                        (key, &ty.args[1], ty.args[1].span)
                    }
                    n => {
                        return Err(TypeError {
                            help: None,
                            message: format!(
                                "'Map' takes 1 or 2 type arguments ('Map<V>' or 'Map<K, V>'), found {n}"
                            ),
                            span: ty.span,
                        })
                    }
                }
            } else {
                if ty.args.len() != 1 {
                    return Err(TypeError {
                        help: None,
                        message: format!(
                            "'{}' takes exactly 1 type argument, found {}",
                            ty.name,
                            ty.args.len()
                        ),
                        span: ty.span,
                    });
                }
                (Ty::Unit, &ty.args[0], ty.args[0].span)
            };
            let elem = self.resolve_element(value_arg)?;
            let (ok, error) = if ty.name == "List" {
                (crate::types::is_list_element(&elem), list_element_error(&elem))
            } else if ty.name == "Map" {
                let enum_value = self.is_enum_ty(&elem);
                let error = if enum_value { map_enum_value_error(&elem) } else { map_element_error(&elem) };
                (!enum_value && crate::types::is_map_element(&elem), error)
            } else if ty.name == "Atomic" {
                (crate::types::is_atomic_element(&elem), atomic_element_error(&elem))
            } else if ty.name == "Thread" {
                (crate::types::is_thread_element(&elem), thread_element_error(&elem))
            } else if ty.name == "Mutex" {
                (crate::types::is_mutex_element(&elem), mutex_element_error(&elem))
            } else if ty.name == "RwLock" {
                (crate::types::is_rwlock_element(&elem), rwlock_element_error(&elem))
            } else if ty.name == "Ptr" {
                (crate::types::is_ptr_element(&elem), crate::types::ptr_element_error(&elem))
            } else {
                (crate::types::is_box_element(&elem), box_element_error(&elem))
            };
            if !ok {
                return Err(TypeError {
                    help: None,
                    message: error,
                    span: value_span,
                });
            }
            let slot = match ty.name.as_str() {
                "List" => Some(managed::HeapSlot::ListElement),
                "Map" => Some(managed::HeapSlot::MapValue),
                "Box" => Some(managed::HeapSlot::BoxPayload),
                _ => None,
            };
            if let Some(slot) = slot {
                note_heap_use(&self.env, slot, &elem, value_span);
            }
            return Ok(if ty.name == "List" {
                Ty::List(Box::new(elem))
            } else if ty.name == "Map" {
                Ty::Map(Box::new(key), Box::new(elem))
            } else if ty.name == "Atomic" {
                Ty::Atomic(Box::new(elem))
            } else if ty.name == "Thread" {
                Ty::Thread(Box::new(elem))
            } else if ty.name == "Mutex" {
                Ty::Mutex(Box::new(elem))
            } else if ty.name == "RwLock" {
                Ty::RwLock(Box::new(elem))
            } else if ty.name == "Ptr" {
                Ty::Ptr(Box::new(elem))
            } else {
                Ty::Box(Box::new(elem))
            });
        }
        if !ty.args.is_empty() {
            // A user-declared generic type (A-2): `Pair<i32, String>`. The
            // arguments are resolved first (they are types in their own
            // right), then the declaration is instantiated for them.
            let qualified = self.qualify(&ty.name, ty.span)?;
            if let Some(tmpl) = self.env.generics.get(&qualified) {
                check_type_exported(
                    &ty.name,
                    self.structs,
                    self.enums,
                    self.aliases,
                    &self.env,
                    ty.span,
                )?;
                if ty.args.len() != tmpl.params.len() {
                    return Err(TypeError {
                        help: None,
                        message: format!(
                            "'{}' takes {} type argument(s), found {}",
                            tmpl.name,
                            tmpl.params.len(),
                            ty.args.len()
                        ),
                        span: ty.span,
                    });
                }
                let mut args = Vec::with_capacity(ty.args.len());
                for (i, a) in ty.args.iter().enumerate() {
                    let declared = tmpl
                        .const_params
                        .iter()
                        .find(|(n, _)| tmpl.params.get(i) == Some(n))
                        .map(|(_, t)| t);
                    args.push(self.resolve_type_arg(a, declared)?);
                }
                return self.instantiate(&qualified, &args, ty.span);
            }
            return Err(TypeError {
                help: None,
                message: format!(
                    "'{}' does not take type arguments: the compiler's generic types are 'List<T>', 'Box<T>', 'Map<V>', 'Atomic<T>', 'Thread<T>', 'Mutex<T>', 'RwLock<T>' and 'Ptr<T>', and a user type has to be declared with parameters to take them ('datac {name}<T> ...')",
                    ty.name,
                    name = ty.name
                ),
                span: ty.span,
            });
        }
        if let Some(builtin) = Ty::from_name(&ty.name) {
            return Ok(builtin);
        }
        // A name that is already a known `struct` key is canonical: this is
        // how a receiver declaration (`fun Circle.area()` inside
        // `shapes/circle.kp`) arrives here, since `resolve.rs` rewrites the
        // receiver type to its module-prefixed name when it merges modules.
        // Only the declaring module's own keys may take that shortcut. A
        // canonical key of *another* module is how that module's type is
        // written through an alias (`util.Tally`), and it has to take the
        // same qualifying and visibility path every other written name takes
        // (A-13). Skipping it is what let a private type cross the boundary.
        if self.structs.contains_key(&ty.name) && self.owns_canonical_key(&ty.name) {
            return Ok(Ty::Struct(ty.name.clone()));
        }
        // The built-in `BlockError` (decision 139) is named with no import, the
        // way a block's own `Result<T, BlockError>` names it.
        if ty.name == "BlockError" && ty.args.is_empty() {
            return Ok(block_error_ty());
        }
        let qualified = self.qualify(&ty.name, ty.span)?;
        if self.handles.contains(&qualified) {
            check_type_exported(
                &ty.name,
                self.structs,
                self.enums,
                self.aliases,
                &self.env,
                ty.span,
            )?;
            // Always a pointer, so `indirect` has nothing to decide: a handle
            // is complete the moment it is declared, and naming one by value
            // is naming a pointer.
            Ok(Ty::Handle(qualified))
        } else if self.structs.contains_key(&qualified) {
            check_type_exported(
                &ty.name,
                self.structs,
                self.enums,
                self.aliases,
                &self.env,
                ty.span,
            )?;
            Ok(Ty::Struct(qualified))
        } else if let Some(tmpl) = self
            .env
            .generics
            .get(&qualified)
            .or_else(|| self.env.generics.get(&ty.name))
        {
            check_type_exported(
                &ty.name,
                self.structs,
                self.enums,
                self.aliases,
                &self.env,
                ty.span,
            )?;
            // A generic type written without its arguments. The generic half
            // of the name is enough to say what is missing.
            Err(TypeError {
                help: None,
                message: format!(
                    "'{}' is a generic type - it needs {} type argument(s): '{}<{}>'",
                    tmpl.name,
                    tmpl.params.len(),
                    tmpl.name,
                    tmpl.params.join(", ")
                ),
                span: ty.span,
            })
        } else if self
            .later_datacs
            .is_some_and(|later| later.contains(&qualified))
        {
            // Forward reference. Legal through a container (M8: a pointer is
            // complete before either `datac` is, so the pair can be
            // mutually recursive), rejected by value, which is the
            // infinite-size case and the reason the struct table is built in
            // declaration order at all.
            if indirect {
                Ok(Ty::Struct(qualified))
            } else {
                Err(TypeError {
                    help: None,
                    message: format!(
                        "'{}' is declared after this type - a type may only name types declared before it by value; behind a 'Box' or 'List' it may name any type, including itself",
                        ty.name
                    ),
                    span: ty.span,
                })
            }
        } else {
            Err(TypeError {
                help: crate::foreign::hint(&ty.name),
                message: match qualified_hint(
                    &ty.name,
                    self.structs.keys(),
                    self.aliases,
                    &self.env,
                ) {
                    Some(hint) => format!("unknown type '{}' - did you mean {hint}?", ty.name),
                    None => format!("unknown type '{}'", ty.name),
                },
                span: ty.span,
            })
        }
    }

    /// Resolves one element (or type argument) of a container: an array's
    /// element, a `List`/`Box` element, a `Map` key or value. Its own `&` is
    /// refused here rather than dropped, because `resolve_base` has no position
    /// to read it in and a silently-ignored `&` turns `List<&i32>` into
    /// `List<i32>`, a written borrow that means something else entirely.
    pub(crate) fn resolve_element(&self, ty: &ast::TypeExpr) -> Result<Ty> {
        if ty.amp {
            return Err(TypeError {
                help: None,
                message: format!(
                    "'&{}' cannot be a container element: an element has to outlive the container that holds it, and v1 has no lifetime syntax to say so; use the owned type",
                    ty.display()
                ),
                span: ty.span,
            });
        }
        self.resolve(ty, true)
    }

    /// Folds the length of an array type or of a repeat literal (A-19).
    pub(crate) fn array_len(&self, expr: &ast::Expr) -> Result<u64> {
        // A const parameter as a length (A-20): `[T; N]` inside a template,
        // where the instantiation binds `N` to a value.
        if let ast::Expr::Ident(name, _) = expr {
            if let Some(Ty::ConstInt { value, ty }) = self.params.get(name) {
                if matches!(**ty, Ty::Char) {
                    return Err(TypeError {
                        help: None,
                        message: format!(
                            "'{name}' is a 'char' const parameter, and an array length has to be an integer"
                        ),
                        span: expr.span(),
                    });
                }
                if *value < 1 {
                    return Err(TypeError {
                        help: None,
                        message: format!(
                            "an array length has to be at least 1, and '{name}' is {value}: v1 has no zero-length form ('[T; 0]' would be a C array with no elements, which is not standard C)"
                        ),
                        span: expr.span(),
                    });
                }
                return Ok(*value as u64);
            }
        }
        // A const parameter anywhere else in a length is arithmetic on a
        // const parameter in a type, which is not in this slice (A-20):
        // refuse it by name rather than folding it.
        if let Some(name) = const_param_in(expr, self.params) {
            return Err(TypeError {
                help: None,
                message: format!(
                    "'{name}' is a const parameter, and arithmetic on one in a type ('[T; {name} + 1]') is not in this slice: write the length the instantiation binds ('[T; {name}]'), or compute it from a 'const' ('const M: usize = ...')"
                ),
                span: expr.span(),
            });
        }
        let (Some(consts), Some(const_fns)) = (self.consts, self.const_fns) else {
            return Err(TypeError {
                help: None,
                message: "an array length has to be a compile-time constant, and this position has no constants in reach".to_string(),
                span: expr.span(),
            });
        };
        ConstEvaluator::new(const_fns, self.structs, &self.enums, &self.env, consts).array_len(
            expr,
            self.prefix,
            self.aliases,
        )
    }

    /// One type-argument position of a generic instantiation (A-20). When the
    /// parameter there is a `const`, the written argument is folded with the
    /// parameter's declared type; otherwise it resolves as a type and a
    /// constant expression written in its place is refused.
    pub(crate) fn resolve_type_arg(&self, ty: &ast::TypeExpr, declared: Option<&Ty>) -> Result<Ty> {
        match declared {
            None => {
                if ty.const_expr.is_some() {
                    return Err(TypeError {
                        help: None,
                        message: format!(
                            "a type argument expected a type, but '{}' is a constant expression: declare the parameter 'const' ('datac Buf<const N: usize>') where a value belongs",
                            ty.display()
                        ),
                        span: ty.span,
                    });
                }
                self.resolve(ty, true)
            }
            Some(declared) => {
                let expr = ty
                    .const_expr
                    .as_deref()
                    .cloned()
                    .or_else(|| named_const_expr(ty));
                let Some(expr) = expr else {
                    return Err(TypeError {
                        help: None,
                        message: format!(
                            "a 'const' parameter expects a constant expression of type '{}', but the type '{}' was written: write a literal or a 'const' ('Buf<16>', 'Buf<SIZE>')",
                            declared.display(),
                            ty.display()
                        ),
                        span: ty.span,
                    });
                };
                self.fold_const_arg(&expr, declared)
            }
        }
    }

    /// Folds one const argument to a value of the parameter's declared type
    /// (A-20). The existing constant evaluation is the whole of it, so a
    /// literal, a `const`, a module `const` and a plain `fun` call all work
    /// exactly as they do in a `const` initializer.
    pub(crate) fn fold_const_arg(&self, expr: &ast::Expr, declared: &Ty) -> Result<Ty> {
        // A const parameter written as a const argument (`Ring<N>` in the
        // template's own signature, A-20): its value is what the enclosing
        // instantiation bound, re-typed to the parameter it is being passed
        // to.
        if let ast::Expr::Ident(name, span) = expr {
            if let Some(Ty::ConstInt { value, ty: source }) = self.params.get(name) {
                let value = *value;
                if matches!(**source, Ty::Char) != matches!(declared, Ty::Char) {
                    return Err(TypeError {
                        help: None,
                        message: format!(
                            "const parameter '{name}' is a '{}' and cannot be passed where a '{}' is expected",
                            source.display(),
                            declared.display()
                        ),
                        span: *span,
                    });
                }
                if !matches!(declared, Ty::Bool | Ty::Char) {
                    check_int_literal(value, declared, false, *span)?;
                }
                return Ok(Ty::ConstInt {
                    value,
                    ty: Box::new(declared.clone()),
                });
            }
        }
        let (Some(consts), Some(const_fns)) = (self.consts, self.const_fns) else {
            return Err(TypeError {
                help: None,
                message: "a const argument has to be a compile-time constant, and this position has no constants in reach".to_string(),
                span: expr.span(),
            });
        };
        let evaluator =
            ConstEvaluator::new(const_fns, self.structs, &self.enums, &self.env, consts);
        let mut scope = ConstScope::new(self.prefix.to_string(), self.aliases.clone());
        let (value, _) = evaluator.eval(expr, Some(declared), &mut scope)?;
        let value = match (value, declared) {
            (ConstValue::Char(c), Ty::Char) => i128::from(c),
            (ConstValue::Int(n), t) if !matches!(t, Ty::Char) => n,
            (ConstValue::Bool(b), t) if !matches!(t, Ty::Char) => i128::from(b),
            _ => {
                return Err(TypeError {
                    help: None,
                    message: format!(
                        "a const parameter of type '{}' takes {}, and this expression is not one",
                        declared.display(),
                        if matches!(declared, Ty::Char) {
                            "a character value"
                        } else {
                            "an integer or boolean value"
                        }
                    ),
                    span: expr.span(),
                })
            }
        };
        Ok(Ty::ConstInt {
            value,
            ty: Box::new(declared.clone()),
        })
    }

    /// Folds a const argument with no parameter type to steer it (A-20): used
    /// by the explicit-instantiation path, which resolves its arguments before
    /// it knows which template they belong to. The folded result carries the
    /// expression's own type, and `bind_const_args` re-types it where the
    /// declaration is known.
    pub(crate) fn fold_const_arg_any(&self, expr: &ast::Expr) -> Result<Ty> {
        let (Some(consts), Some(const_fns)) = (self.consts, self.const_fns) else {
            return Err(TypeError {
                help: None,
                message: "a const argument has to be a compile-time constant, and this position has no constants in reach".to_string(),
                span: expr.span(),
            });
        };
        let evaluator =
            ConstEvaluator::new(const_fns, self.structs, &self.enums, &self.env, consts);
        let mut scope = ConstScope::new(self.prefix.to_string(), self.aliases.clone());
        let (value, ty) = evaluator.eval(expr, None, &mut scope)?;
        let (value, ty) = match value {
            ConstValue::Int(n) => (n, ty),
            ConstValue::Bool(b) => (i128::from(b), ty),
            ConstValue::Char(c) => (i128::from(c), Ty::Char),
            _ => {
                return Err(TypeError {
                    help: None,
                    message: "a const argument has to be an integer, boolean or character value".to_string(),
                    span: expr.span(),
                })
            }
        };
        Ok(Ty::ConstInt {
            value,
            ty: Box::new(ty),
        })
    }

    /// A written name as its canonical, module-qualified key: a bare name
    /// belongs to the current module, and `alias.Item` resolves through the
    /// module's `impl` table. Factored out of `resolve_base` so the generic
    /// path and the ordinary one qualify a name exactly the same way.
    pub(crate) fn qualify(&self, name: &str, span: Span) -> Result<String> {
        match name.split_once('.') {
            Some((alias, item)) => match self.aliases.get(alias) {
                Some(prefix) => Ok(format!("{prefix}{item}")),
                None => Err(TypeError {
                    help: None,
                    message: format!(
                        "unknown module alias '{alias}' in type '{name}' - was it imported?"
                    ),
                    span,
                }),
            },
            None => Ok(format!("{}{}", self.prefix, name)),
        }
    }

    /// Builds (or looks up) the concrete type for one instantiation of a
    /// generic template (A-2). The instance is registered *before* its fields
    /// are resolved only as an in-progress mark, so a recursive field
    /// (`Box<Node<i32>>` inside `Node<i32>`) terminates on the key rather
    /// than recursing forever; the finished definition is stored afterwards.
    pub(crate) fn instantiate(&self, name: &str, args: &[Ty], span: Span) -> Result<Ty> {
        // A-20: a const argument is re-typed to its parameter's declared type
        // (and range-checked) before anything keys off it, so the instance key
        // and every later binding see the same value.
        let mut args = args.to_vec();
        let tmpl = self.env.generics[name].clone();
        bind_const_args(&tmpl.params, &tmpl.const_params, &mut args, span)?;
        let args = args.as_slice();
        let key = instance_key(name, args);
        {
            let st = self.env.instances.borrow();
            if st.records.contains_key(&key) || st.sums.contains_key(&key) {
                return Ok(Ty::Struct(key));
            }
        }
        {
            let mut st = self.env.instances.borrow_mut();
            if !st.in_progress.insert(key.clone()) {
                return Ok(Ty::Struct(key));
            }
        }
        let params: HashMap<String, Ty> = tmpl
            .params
            .iter()
            .cloned()
            .zip(args.iter().cloned())
            .collect();
        let scope = TypeScope {
            structs: self.structs,
            enums: self.enums,
            env: self.env.clone(),
            params: &params,
            prefix: &tmpl.prefix,
            aliases: &tmpl.aliases,
            later_datacs: None,
            in_progress: Some(&key),
            consts: self.consts,
            const_fns: self.const_fns,
            handles: self.handles,
        };
        let built = scope.build_instance(&tmpl, &key);
        self.env.instances.borrow_mut().in_progress.remove(&key);
        let inst = built.map_err(|e| TypeError {
            help: None,
            // A template's fields live in the template's own file, so an
            // error found while resolving them cannot keep its own span: the
            // caller attributes the error to *its* module, which is where
            // the instantiation was written. Re-point the span at the use
            // site and name the instance in the message instead of printing
            // a location in the wrong file.
            message: format!("in '{}': {}", key, e.message),
            span,
        })?;
        let mut st = self.env.instances.borrow_mut();
        match inst {
            Instance::Record(s) => {
                st.records.insert(key.clone(), s);
            }
            Instance::Sum(e) => {
                st.sums.insert(key.clone(), e);
            }
        }
        st.origin
            .insert(key.clone(), (name.to_string(), args.to_vec()));
        Ok(Ty::Struct(key))
    }

    /// Resolves a template's fields with its parameters bound, producing one
    /// concrete definition. Validation the non-generic path does at
    /// declaration time (duplicate field names, borrowed fields, duplicate
    /// variants) happens here instead, because a field type only means
    /// something once the parameters are bound.
    pub(crate) fn build_instance(&self, tmpl: &GenericTemplate, key: &str) -> Result<Instance> {
        match &tmpl.kind {
            GenericKind::Record(fields) => {
                let mut out: Vec<FieldDef> = Vec::new();
                for f in fields {
                    if out.iter().any(|fd| fd.name == f.name) {
                        return Err(TypeError {
                            help: None,
                            message: format!(
                                "field '{}' is defined more than once in datac '{}'",
                                f.name, tmpl.name
                            ),
                            span: f.span,
                        });
                    }
                    let ty = self.resolve(&f.ty, false)?;
                    if crate::types::is_borrowed_field_ty(&ty) {
                        return Err(TypeError {
                            help: None,
                            message: format!(
                                "field '{}' is a borrowed type ('{}') - a datac holds owned data only in v1, so a '&str', 'View' or reference field is not expressible - an owned 'String' is the way to keep text in one",
                                f.name,
                                ty.display()
                            ),
                            span: f.span,
                        });
                    }
                    out.push(FieldDef {
                        name: f.name.clone(),
                        ty,
                        mutable: f.mutable,
                    });
                }
                Ok(Instance::Record(StructDef {
                    name: key.to_string(),
                    fields: out,
                    repr: tmpl.repr.clone(),
                    // Reassigned by `order_declarations` once every
                    // instantiation is known; the value here is a sentinel.
                    order: usize::MAX,
                }))
            }
            GenericKind::Sum(variants) => {
                let mut out: Vec<crate::types::VariantDef> = Vec::new();
                for v in variants {
                    if out.iter().any(|vd| vd.name == v.name) {
                        return Err(TypeError {
                            help: None,
                            message: format!(
                                "variant '{}' is defined more than once in enum '{}'",
                                v.name, tmpl.name
                            ),
                            span: v.span,
                        });
                    }
                    let mut fields: Vec<FieldDef> = Vec::new();
                    for f in &v.fields {
                        if fields.iter().any(|fd| fd.name == f.name) {
                            return Err(TypeError {
                                help: None,
                                message: format!(
                                    "payload field '{}' is defined more than once in variant '{}'",
                                    f.name, v.name
                                ),
                                span: f.span,
                            });
                        }
                        let ty = self.resolve(&f.ty, false)?;
                        if crate::types::is_borrowed_field_ty(&ty) {
                            return Err(TypeError {
                                help: None,
                                message: format!(
                                    "payload field '{}' is a borrowed type ('{}') - an enum holds owned data only in v1, so a '&str', 'View' or reference payload is not expressible - an owned 'String' is the way to keep text in one",
                                    f.name,
                                    ty.display()
                                ),
                                span: f.span,
                            });
                        }
                        fields.push(FieldDef {
                            name: f.name.clone(),
                            ty,
                            mutable: f.mutable,
                        });
                    }
                    out.push(crate::types::VariantDef {
                        name: v.name.clone(),
                        fields,
                        discriminant: v.discriminant,
                    });
                }
                Ok(Instance::Sum(EnumDef {
                    name: key.to_string(),
                    variants: out,
                    repr: tmpl.repr.clone(),
                    order: usize::MAX,
                }))
            }
        }
    }
}

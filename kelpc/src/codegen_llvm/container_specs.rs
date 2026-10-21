use super::*;

impl<'a> Gen<'a> {
    /// The one list of functions each container kind is emitted with, gathered
    /// before any body so a callee is always declared first.
    pub(crate) fn list_specs(&self, elem: &Ty) -> Result<Vec<CfnSpec<'a>>, String> {
        let base = list_c_type(elem);
        let list = self.cty(&Ty::List(Box::new(elem.clone())))?;
        let elem_ty = self.lty2(elem)?;
        let ptr = self.pty();
        let void = self.ctx.void();
        let (i1t, i32t, i64t) = (self.ctx.i1(), self.ctx.i32(), self.ctx.i64());
        let sortable = crate::types::value_is_sortable(elem, &self.sortable);
        let clonable = crate::types::value_is_clonable(elem, &self.clonable);
        let comparable = crate::types::list_comparable_elem(elem);
        let mut out = vec![
            spec(&format!("{base}__empty"), list, &[]),
            spec(&format!("{base}__reserve"), void, &[ptr, i64t]),
            spec(&format!("{base}__with_capacity"), list, &[i64t]),
            spec(&format!("{base}__of"), list, &[elem_ty]),
            spec(&format!("{base}__len"), i64t, &[ptr]),
            spec(&format!("{base}__is_empty"), i1t, &[ptr]),
            spec(&format!("{base}__push"), void, &[ptr, elem_ty]),
            spec(&format!("{base}__bounds"), void, &[ptr, i64t]),
        ];
        if elem.is_copy() {
            out.push(spec(&format!("{base}__get"), elem_ty, &[ptr, i64t]));
        } else {
            out.push(spec(&format!("{base}__get_const"), ptr, &[ptr, i64t]));
        }
        out.push(spec(&format!("{base}__set"), void, &[ptr, i64t, elem_ty]));
        out.push(spec(&format!("{base}__pop"), elem_ty, &[ptr]));
        out.push(spec(&format!("{base}__clear"), void, &[ptr]));
        out.push(spec(&format!("{base}__reverse"), void, &[ptr]));
        if sortable {
            out.push(spec(&format!("{base}__cmp"), i32t, &[ptr, ptr]));
            out.push(spec(
                &format!("{base}__sift_down"),
                void,
                &[ptr, i64t, i64t, i64t],
            ));
            out.push(spec(
                &format!("{base}__heapsort_range"),
                void,
                &[ptr, i64t, i64t],
            ));
            out.push(spec(
                &format!("{base}__insertion_sort"),
                void,
                &[ptr, i64t, i64t],
            ));
            out.push(spec(
                &format!("{base}__median3"),
                i64t,
                &[ptr, i64t, i64t, i64t],
            ));
            out.push(spec(
                &format!("{base}__partition"),
                i64t,
                &[ptr, i64t, i64t],
            ));
            out.push(spec(
                &format!("{base}__quicksort"),
                void,
                &[ptr, i64t, i64t, i64t],
            ));
            out.push(spec(&format!("{base}__sort"), void, &[ptr]));
        }
        // `contains` is gated on equality, which is a wider set than order in
        // the earlier emitter only for a `datac` element: `list_comparable_elem` is
        // the same set as `list_sortable_elem`, so a `datac` element gets
        // `sort` but never `contains`.
        if comparable {
            // A `String`/`Bytes` element is compared through a shared borrow
            // (`contains_param`), every other comparable element by value.
            let contains_param = if elem.is_copy() { elem_ty } else { ptr };
            out.push(spec(&format!("{base}__contains"), i1t, &[ptr, contains_param]));
        }
        if clonable {
            out.push(spec(&format!("{base}__slice"), list, &[ptr, i64t, i64t]));
            out.push(spec(&format!("{base}__clone"), list, &[ptr]));
        }
        out.push(spec(&format!("{base}__insert"), void, &[ptr, i64t, elem_ty]));
        out.push(spec(&format!("{base}__remove"), elem_ty, &[ptr, i64t]));
        out.push(spec(
            &drop_fn_name(&Ty::List(Box::new(elem.clone()))),
            void,
            &[ptr],
        ));
        Ok(out)
    }

    pub(crate) fn box_specs(&self, elem: &Ty) -> Result<Vec<CfnSpec<'a>>, String> {
        let base = box_c_type(elem);
        let boxed = self.cty(&Ty::Box(Box::new(elem.clone())))?;
        let elem_ty = self.lty2(elem)?;
        let ptr = self.pty();
        let void = self.ctx.void();
        let i1t = self.ctx.i1();
        let mut out = vec![
            spec(&format!("{base}__empty"), boxed, &[]),
            spec(&format!("{base}__of"), boxed, &[elem_ty]),
            spec(&format!("{base}__is_empty"), i1t, &[ptr]),
        ];
        if elem.is_copy() {
            out.push(spec(&format!("{base}__get"), elem_ty, &[ptr]));
        } else {
            out.push(spec(&format!("{base}__get_const"), ptr, &[ptr]));
        }
        out.push(spec(&format!("{base}__set"), void, &[ptr, elem_ty]));
        out.push(spec(&format!("{base}__take"), elem_ty, &[ptr]));
        out.push(spec(
            &drop_fn_name(&Ty::Box(Box::new(elem.clone()))),
            void,
            &[ptr],
        ));
        Ok(out)
    }

    pub(crate) fn array_specs(&self, elem: &Ty, len: u64) -> Result<Vec<CfnSpec<'a>>, String> {
        let base = crate::types::array_c_type(elem, len);
        let array = self.cty(&Ty::Array(Box::new(elem.clone()), len))?;
        let elem_ty = self.lty2(elem)?;
        let ptr = self.pty();
        let void = self.ctx.void();
        let (i1t, i64t) = (self.ctx.i1(), self.ctx.i64());
        let mut out = vec![
            spec(&format!("{base}__len"), i64t, &[ptr]),
            spec(&format!("{base}__is_empty"), i1t, &[ptr]),
            spec(&format!("{base}__bounds"), void, &[i64t]),
        ];
        if elem.is_copy() {
            out.push(spec(&format!("{base}__get"), elem_ty, &[ptr, i64t]));
        } else {
            out.push(spec(&format!("{base}__get_const"), ptr, &[ptr, i64t]));
        }
        out.push(spec(&format!("{base}__set"), void, &[ptr, i64t, elem_ty]));
        out.push(spec(&format!("{base}__fill"), array, &[elem_ty]));
        // Kernel stage 14: the in-place fill a `[v; N]` local uses, so a large
        // repeat never becomes an aggregate value that LLVM's instcombine then
        // folds element by element.
        out.push(spec(&format!("{base}__fill_into"), void, &[ptr, elem_ty]));
        out.push(spec(
            &drop_fn_name(&Ty::Array(Box::new(elem.clone()), len)),
            void,
            &[ptr],
        ));
        Ok(out)
    }

    pub(crate) fn declare_container_fns(&mut self) -> Result<(), String> {
        let mut specs: Vec<CfnSpec<'a>> = Vec::new();
        for elem in self.lists_sorted() {
            specs.extend(self.list_specs(&elem)?);
        }
        for elem in self.boxes_sorted() {
            specs.extend(self.box_specs(&elem)?);
        }
        for (key, value) in self.maps_sorted() {
            specs.extend(self.map_specs(&key, &value)?);
        }
        for arr in self.arrays_sorted() {
            if let Ty::Array(elem, len) = arr {
                specs.extend(self.array_specs(&elem, len)?);
            }
        }
        for s in specs {
            let ty = self.ctx.fn_type(s.ret, &s.params, false);
            let func = self.module.add_function(&s.symbol, ty);
            // C's `static`: the body is emitted into this module just below,
            // and an external symbol is one LLVM will not inline into its
            // callers, which is why the earlier emitter's copy of these bodies runs
            // 1.15x faster than this backend's. No runtime body names a
            // container symbol, so nothing outside the module calls one. In
            // the incremental mode the `_support` unit defines them and every
            // other unit calls them, so they stay external there (roadmap
            // A-14 slice 1).
            if self.unit.is_none() {
                func.as_value().set_internal_linkage();
            }
            self.container_fns.insert(s.symbol, (func, ty));
        }
        Ok(())
    }

    pub(crate) fn lists_sorted(&self) -> Vec<Ty> {
        let mut v: Vec<Ty> = self.instantiations.lists.iter().cloned().collect();
        v.sort_by_key(|t| (container_depth(t), container_sort_name(t)));
        v
    }

    pub(crate) fn boxes_sorted(&self) -> Vec<Ty> {
        let mut v: Vec<Ty> = self.instantiations.boxes.iter().cloned().collect();
        v.sort_by_key(|t| (container_depth(t), container_sort_name(t)));
        v
    }

    pub(crate) fn maps_sorted(&self) -> Vec<(Ty, Ty)> {
        let mut v: Vec<(Ty, Ty)> = self.instantiations.maps.iter().cloned().collect();
        v.sort_by_key(|(k, val)| format!("{}{}", container_sort_name(k), container_sort_name(val)));
        v
    }

    pub(crate) fn arrays_sorted(&self) -> Vec<Ty> {
        let mut v: Vec<Ty> = self.instantiations.arrays.iter().cloned().collect();
        v.sort_by_key(|t| (container_depth(t), container_sort_name(t)));
        v
    }
}

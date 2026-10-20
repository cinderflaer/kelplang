use super::*;

impl<'a> Gen<'a> {
    /// The address of the value cell an entry holds: `&entries[e].value`,
    /// where `slot` is the index-slot result `find` returned
    /// (`entries[slot - 1]`).
    pub(crate) fn map_value_ptr(
        &self,
        map_ty: Type<'a>,
        entry_ty: Type<'a>,
        self_p: Value<'a>,
        slot: Value<'a>,
    ) -> Result<Value<'a>, String> {
        // `slot` is the *index* slot `find` returned; the entry it names is
        // `index[slot] - 1`, exactly the C `self->index[slot] - 1`.
        let index = self.ld(self.pty(), self.fld(map_ty, self_p, M_INDEX, "index"), "index");
        let holder = self.ld(self.ctx.i32(), self.gep(self.ctx.i32(), index, slot, "at"), "holder");
        let holder64 = self.builder.build_int_cast(false, true, holder, self.ctx.i64(), "holder64");
        let e = self.builder.build_int_binop(IntBinOp::Sub, holder64, self.v_i64(1), "e");
        let entries = self.ld(self.pty(), self.fld(map_ty, self_p, M_ENTRIES, "entries"), "entries");
        let entry = self.gep(entry_ty, entries, e, "entry");
        Ok(self.fld(entry_ty, entry, E_VALUE, "value"))
    }
}

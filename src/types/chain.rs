use ckb_types::packed;

#[derive(Clone)]
pub(crate) struct ScriptAnchor {
    cell_dep: packed::CellDep,
    data_hash: packed::Byte32,
    type_hash: packed::Byte32,
}

impl ScriptAnchor {
    pub(crate) fn new(
        cell_dep: packed::CellDep,
        data_hash: packed::Byte32,
        type_hash: packed::Byte32,
    ) -> Self {
        Self {
            cell_dep,
            data_hash,
            type_hash,
        }
    }

    pub(crate) fn cell_dep(&self) -> packed::CellDep {
        self.cell_dep.clone()
    }

    pub(crate) fn data_hash(&self) -> packed::Byte32 {
        self.data_hash.clone()
    }

    pub(crate) fn type_hash(&self) -> packed::Byte32 {
        self.type_hash.clone()
    }
}

use super::*;

impl HistoryCells {
    fn retained_bytes(&self) -> usize {
        match self {
            Self::Plain(cells) => cells
                .capacity()
                .saturating_mul(std::mem::size_of::<PlainHistoryCell>()),
            Self::Metadata(cells) => cells
                .capacity()
                .saturating_mul(std::mem::size_of::<MetadataHistoryCell>()),
            Self::Dense(cells) => cells.capacity().saturating_mul(std::mem::size_of::<Cell>()),
        }
    }
}

impl HistoryRow {
    pub(super) fn stored_len(&self) -> usize {
        self.cells.len()
    }

    pub(super) fn retained_bytes(&self) -> usize {
        self.cells.retained_bytes()
    }

    fn plain_capacity(&self) -> usize {
        match &self.cells {
            HistoryCells::Plain(cells) => cells.capacity(),
            _ => 0,
        }
    }

    fn metadata_capacity(&self) -> usize {
        match &self.cells {
            HistoryCells::Metadata(cells) => cells.capacity(),
            _ => 0,
        }
    }

    fn dense_capacity(&self) -> usize {
        match &self.cells {
            HistoryCells::Dense(cells) => cells.capacity(),
            _ => 0,
        }
    }
}

impl std::ops::Index<usize> for HistoryRow {
    type Output = Cell;

    fn index(&self, index: usize) -> &Self::Output {
        assert!(index < self.len(), "history cell index is in bounds");
        match &self.cells {
            HistoryCells::Dense(cells) => cells.get(index).unwrap_or(&DEFAULT_CELL),
            HistoryCells::Plain(cells) if index >= cells.len() => &DEFAULT_CELL,
            HistoryCells::Metadata(cells) if index >= cells.len() => &DEFAULT_CELL,
            _ => panic!("compact history cells must be read through HistoryRow::get"),
        }
    }
}

impl RowView<'_> {
    pub(crate) fn to_vec(self) -> Vec<Cell> {
        let mut cells = Vec::with_capacity(self.len());
        self.append_to(&mut cells, self.len());
        cells
    }
}

impl std::ops::Index<usize> for RowView<'_> {
    type Output = Cell;

    fn index(&self, index: usize) -> &Self::Output {
        match self {
            Self::Dense(cells) => &cells[index],
            Self::History(row) => &row[index],
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub(super) struct HistoryStorageStats {
    pub(super) rows: usize,
    pub(super) logical_cells: usize,
    pub(super) plain_capacity: usize,
    pub(super) metadata_capacity: usize,
    pub(super) dense_capacity: usize,
    pub(super) retained_bytes: usize,
}

impl Grid {
    pub(super) fn history_storage_stats(&self) -> HistoryStorageStats {
        HistoryStorageStats {
            rows: self.history.len(),
            logical_cells: self.history.iter().map(HistoryRow::len).sum(),
            plain_capacity: self.history.iter().map(HistoryRow::plain_capacity).sum(),
            metadata_capacity: self.history.iter().map(HistoryRow::metadata_capacity).sum(),
            dense_capacity: self.history.iter().map(HistoryRow::dense_capacity).sum(),
            retained_bytes: self
                .history
                .capacity()
                .saturating_mul(std::mem::size_of::<HistoryRow>())
                .saturating_add(self.history.iter().map(HistoryRow::retained_bytes).sum()),
        }
    }
}

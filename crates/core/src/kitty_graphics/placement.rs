use super::*;

impl KittyGraphicsState {
    pub(super) fn placement_by_key(
        &self,
        screen: KittyGraphicsScreen,
        image_id: u32,
        placement_id: u32,
    ) -> Option<&Placement> {
        self.placements.iter().find(|placement| {
            placement.screen == screen
                && placement.image_id == image_id
                && (placement_id == 0 || placement.placement_id == placement_id)
        })
    }

    pub(super) fn validate_relative_parent(
        &self,
        screen: KittyGraphicsScreen,
        image_id: u32,
        placement_id: u32,
        parent_image_id: u32,
        parent_placement_id: u32,
    ) -> Result<(), String> {
        if image_id == parent_image_id && placement_id == parent_placement_id {
            return Err("EINVAL:a placement cannot be relative to itself".into());
        }
        let mut parent = self
            .placement_by_key(screen, parent_image_id, parent_placement_id)
            .ok_or_else(|| "ENOPARENT:relative placement parent not found".to_string())?;
        for depth in 1..=MAX_RELATIVE_DEPTH {
            let PlacementLocation::Relative {
                parent_image_id,
                parent_placement_id,
                ..
            } = parent.location
            else {
                return Ok(());
            };
            if parent_image_id == image_id && parent_placement_id == placement_id {
                return Err("ECYCLE:relative placement cycle".into());
            }
            parent = self
                .placement_by_key(screen, parent_image_id, parent_placement_id)
                .ok_or_else(|| "ENOPARENT:relative placement parent not found".to_string())?;
            if depth == MAX_RELATIVE_DEPTH {
                return Err("ETOODEEP:relative placement chain is too deep".into());
            }
        }
        unreachable!()
    }

    pub(super) fn resolve_render_origin(
        &self,
        placement: &Placement,
        placeholders: &[KittyGraphicsPlaceholder],
    ) -> Option<ResolvedOrigin> {
        let mut current = placement;
        let mut horizontal_offset = 0i64;
        let mut vertical_offset = 0i64;
        let relative = matches!(placement.location, PlacementLocation::Relative { .. });
        for _ in 0..=MAX_RELATIVE_DEPTH {
            match current.location {
                PlacementLocation::Direct { anchor_line, col } => {
                    let col = i64::try_from(col)
                        .unwrap_or(i64::MAX)
                        .saturating_add(horizontal_offset)
                        .max(0);
                    return Some(ResolvedOrigin::Buffer {
                        anchor_line: anchor_line.saturating_add(vertical_offset),
                        col: usize::try_from(col).unwrap_or(usize::MAX),
                    });
                }
                PlacementLocation::Virtual => {
                    let matching = placeholders.iter().filter(|placeholder| {
                        placeholder.image_id == current.image_id
                            && (current.placement_id == 0
                                || placeholder.placement_id == current.placement_id)
                    });
                    let (row, col) = if relative {
                        matching.fold(None, |origin, placeholder| {
                            let candidate = (placeholder.viewport_row, placeholder.col);
                            Some(origin.map_or(candidate, |(row, col): (i64, usize)| {
                                (row.min(candidate.0), col.min(candidate.1))
                            }))
                        })?
                    } else {
                        matching.fold(None, |origin, placeholder| {
                            let row = placeholder
                                .viewport_row
                                .saturating_sub(i64::from(placeholder.image_row));
                            let col = placeholder
                                .col
                                .saturating_sub(placeholder.image_col as usize);
                            Some(
                                origin.map_or((row, col), |(old_row, old_col): (i64, usize)| {
                                    (old_row.min(row), old_col.min(col))
                                }),
                            )
                        })?
                    };
                    let col = i64::try_from(col)
                        .unwrap_or(i64::MAX)
                        .saturating_add(horizontal_offset)
                        .max(0);
                    return Some(ResolvedOrigin::Viewport {
                        row: row.saturating_add(vertical_offset),
                        col: usize::try_from(col).unwrap_or(usize::MAX),
                    });
                }
                PlacementLocation::Relative {
                    parent_image_id,
                    parent_placement_id,
                    horizontal_offset: horizontal,
                    vertical_offset: vertical,
                } => {
                    horizontal_offset = horizontal_offset.saturating_add(i64::from(horizontal));
                    vertical_offset = vertical_offset.saturating_add(i64::from(vertical));
                    current = self.placement_by_key(
                        placement.screen,
                        parent_image_id,
                        parent_placement_id,
                    )?;
                }
            }
        }
        None
    }

    pub(super) fn remove_orphaned_relative_placements(&mut self) {
        loop {
            let existing = self
                .placements
                .iter()
                .map(|placement| (placement.screen, placement.image_id, placement.placement_id))
                .collect::<Vec<_>>();
            let before = self.placements.len();
            self.placements.retain(|placement| {
                let PlacementLocation::Relative {
                    parent_image_id,
                    parent_placement_id,
                    ..
                } = placement.location
                else {
                    return true;
                };
                existing.iter().any(|(screen, image_id, placement_id)| {
                    *screen == placement.screen
                        && *image_id == parent_image_id
                        && (parent_placement_id == 0 || *placement_id == parent_placement_id)
                })
            });
            if self.placements.len() == before {
                break;
            }
        }
    }
}

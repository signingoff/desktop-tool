//! Pure layout: never move system controls, only use unoccupied horizontal intervals.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Span {
    pub start: i32,
    pub end: i32,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Placement {
    pub x: i32,
    pub width: i32,
    pub visible: usize,
}

#[cfg(test)]
pub fn place(
    width: i32,
    occupied: &[Span],
    cell: i32,
    count: usize,
    margin: i32,
) -> Option<Placement> {
    place_widths(width, occupied, &vec![cell; count], margin)
}

pub fn place_widths(
    width: i32,
    occupied: &[Span],
    widths: &[i32],
    margin: i32,
) -> Option<Placement> {
    if widths.len() < 2
        || widths.iter().any(|&w| w <= 0)
        || margin < 0
        || width <= margin.saturating_mul(2)
    {
        return None;
    }
    let mut spans: Vec<_> = occupied
        .iter()
        .filter(|r| r.end > r.start)
        .map(|r| Span {
            start: r.start.saturating_sub(margin).clamp(margin, width - margin),
            end: r.end.saturating_add(margin).clamp(margin, width - margin),
        })
        .collect();
    spans.sort_by_key(|r| r.start);
    spans.push(Span {
        start: width - margin,
        end: width - margin,
    });
    let mut cursor = margin;
    let mut best = None;
    for span in spans {
        let available = span.start - cursor;
        let mut used = 0i32;
        let visible = widths
            .iter()
            .take_while(|&&w| match used.checked_add(w) {
                Some(next) if next <= available => {
                    used = next;
                    true
                }
                _ => false,
            })
            .count();
        if visible >= 2 && best.is_none_or(|p: Placement| visible >= p.visible) {
            let width = used;
            best = Some(Placement {
                x: span.start - width,
                width,
                visible,
            });
        }
        cursor = cursor.max(span.end);
    }
    best
}

pub fn page(start: usize, visible: usize, count: usize) -> Vec<usize> {
    if count == 0 {
        return Vec::new();
    }
    (0..visible.min(count))
        .map(|n| (start % count + n) % count)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn measured_widths_fit_tightly_and_keep_two_item_boundary() {
        let widths = [83, 117, 91];
        assert!(place_widths(215, &[], &widths, 8).is_none());
        let p = place_widths(216, &[], &widths, 8).unwrap();
        assert_eq!((p.x, p.width, p.visible), (8, 200, 2));
        assert_eq!(place_widths(307, &[], &widths, 8).unwrap().visible, 3);
        assert!(place_widths(400, &[], &[83, 0], 8).is_none());
        assert!(place_widths(i32::MAX, &[], &[i32::MAX - 20, 100], 0).is_none());
    }
    #[test]
    fn fits_all_or_scrolls_without_touching_controls() {
        let occupied = [
            Span { start: 0, end: 100 },
            Span {
                start: 1250,
                end: 1920,
            },
        ];
        let p = place(1920, &occupied, 190, 5, 8).unwrap();
        assert_eq!(p.visible, 5);
        assert!(p.x >= 108);
        assert!(p.x + p.width <= 1242);
        let occupied = [
            Span { start: 0, end: 600 },
            Span {
                start: 1020,
                end: 1920,
            },
        ];
        assert_eq!(place(1920, &occupied, 190, 5, 8).unwrap().visible, 2);
    }
    #[test]
    fn crowded_and_invalid_layouts_hide() {
        // A gap that fits only one index must not produce a one-item ticker.
        assert!(place(395, &[], 190, 5, 8).is_none());
        assert_eq!(place(396, &[], 190, 5, 8).unwrap().visible, 2);
        assert!(place(800, &[Span { start: 0, end: 800 }], 190, 5, 8).is_none());
        assert!(place(
            200,
            &[Span {
                start: 100,
                end: 200
            }],
            190,
            5,
            8
        )
        .is_none());
        assert!(place(0, &[], 190, 5, 8).is_none());
        assert!(place(800, &[], 0, 5, 8).is_none());
    }
    #[test]
    fn overlapping_unsorted_ranges_and_dpi() {
        let occupied = [
            Span {
                start: 400,
                end: 600,
            },
            Span {
                start: -30,
                end: 200,
            },
            Span {
                start: 150,
                end: 450,
            },
        ];
        let p = place(1400, &occupied, 380, 5, 16).unwrap();
        assert!(p.x >= 616);
        assert_eq!(p.visible, 2);
    }
    #[test]
    fn carousel_covers_every_index_without_duplicates() {
        assert_eq!(page(4, 2, 5), vec![4, 0]);
        for count in [3, 5] {
            for visible in 1..=count {
                let pages: Vec<_> = (0..count).flat_map(|s| page(s, visible, count)).collect();
                for i in 0..count {
                    assert!(pages.contains(&i));
                }
                for s in 0..count {
                    let p = page(s, visible, count);
                    assert_eq!(
                        p.iter()
                            .copied()
                            .collect::<std::collections::HashSet<_>>()
                            .len(),
                        visible
                    );
                }
            }
        }
    }
}

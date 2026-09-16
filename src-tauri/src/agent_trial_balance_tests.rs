use super::page_boundary;

#[test]
fn maximum_offset_returns_an_empty_page_without_overflowing() {
    assert_eq!(page_boundary(usize::MAX, 0, 1), (false, None));
    assert_eq!(page_boundary(0, 2, 3), (true, Some(2)));
    assert_eq!(page_boundary(2, 1, 3), (false, None));
}

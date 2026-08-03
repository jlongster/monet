export const scrollTopForRange = (
  viewportStart: number,
  viewportHeight: number,
  rowStart: number,
  rowHeight = 1,
) => {
  if (viewportHeight <= 0) return viewportStart
  const viewportEnd = viewportStart + viewportHeight
  const rowEnd = rowStart + rowHeight
  if (rowStart < viewportStart) return rowStart
  if (rowEnd > viewportEnd) return rowEnd - viewportHeight
  return viewportStart
}

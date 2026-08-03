export const moveOrderedFocus = <Value>(
  values: readonly Value[],
  current: Value,
  direction: -1 | 1,
) => {
  if (values.length === 0) return current
  const currentIndex = values.indexOf(current)
  const startIndex = currentIndex < 0 ? 0 : currentIndex
  return values[(startIndex + direction + values.length) % values.length]!
}

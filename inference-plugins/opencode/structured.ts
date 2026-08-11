export const parseStructured = (text: string) => {
  const trimmed = text.trim()
  try {
    return JSON.parse(trimmed)
  } catch (cause) {
    const fenced = /^```(?:json)?\s*([\s\S]*?)\s*```$/i.exec(trimmed)
    if (!fenced) throw cause
    return JSON.parse(fenced[1].trim())
  }
}

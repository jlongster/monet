export const grayscale = [
  { step: 100, value: "#f5f5f5" },
  { step: 200, value: "#e5e5e5" },
  { step: 300, value: "#d4d4d4" },
  { step: 400, value: "#a3a3a3" },
  { step: 500, value: "#737373" },
  { step: 600, value: "#525252" },
  { step: 700, value: "#404040" },
  { step: 800, value: "#262626" },
  { step: 900, value: "#171717" },
] as const

export const gray = Object.fromEntries(grayscale.map(({ step, value }) => [step, value])) as {
  [Step in (typeof grayscale)[number]["step"]]: (typeof grayscale)[number]["value"]
}

export const selectionBackground = gray[800]

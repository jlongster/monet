import { mkdir, writeFile } from "node:fs/promises"
import { resolve } from "node:path"

import { createCanvas } from "@napi-rs/canvas"
import { TextAttributes, type CapturedFrame, type CapturedSpan, type CliRenderer } from "@opentui/core"
import stringWidth from "string-width"

const CELL_WIDTH = 9
const CELL_HEIGHT = 18
const FONT_SIZE = 14
const PADDING = 18
const PIXEL_RATIO = 2
const segmenter = new Intl.Segmenter(undefined, { granularity: "grapheme" })

export async function captureScreenshot(renderer: CliRenderer): Promise<string> {
  const buffer = renderer.currentRenderBuffer
  const cursor = renderer.getCursorState()
  const frame: CapturedFrame = {
    cols: buffer.width,
    rows: buffer.height,
    cursor: [cursor.x, cursor.y],
    lines: buffer.getSpanLines(),
  }

  const directory = resolve(process.env.MONEY_TUI_SCREENSHOT_DIR ?? ".money-tui/screenshots")
  await mkdir(directory, { recursive: true })
  const timestamp = new Date().toISOString().replaceAll(":", "-").replace(".", "-")
  const filename = resolve(directory, `money-tui-${timestamp}-${crypto.randomUUID().slice(0, 8)}.png`)
  await writeFile(filename, await renderFrame(frame))
  return filename
}

export async function renderFrame(frame: CapturedFrame): Promise<Buffer> {
  const width = frame.cols * CELL_WIDTH + PADDING * 2
  const height = frame.rows * CELL_HEIGHT + PADDING * 2
  const canvas = createCanvas(width * PIXEL_RATIO, height * PIXEL_RATIO)
  const context = canvas.getContext("2d")
  context.scale(PIXEL_RATIO, PIXEL_RATIO)
  context.fillStyle = "#000000"
  context.fillRect(0, 0, width, height)
  context.textBaseline = "middle"

  for (const [row, line] of frame.lines.entries()) {
    let col = 0
    for (const span of line.spans) {
      drawSpan(context, span, col, row)
      col += span.width
    }
  }

  return canvas.encode("png")
}

function drawSpan(
  context: ReturnType<ReturnType<typeof createCanvas>["getContext"]>,
  span: CapturedSpan,
  col: number,
  row: number,
) {
  const inverse = (span.attributes & TextAttributes.INVERSE) !== 0
  const foreground = inverse ? span.bg : span.fg
  const background = inverse ? span.fg : span.bg
  const x = PADDING + col * CELL_WIDTH
  const y = PADDING + row * CELL_HEIGHT
  const spanWidth = span.width * CELL_WIDTH

  context.fillStyle = cssColor(background)
  context.fillRect(x, y, spanWidth, CELL_HEIGHT)

  if ((span.attributes & TextAttributes.HIDDEN) !== 0 || !span.text.trimEnd()) return

  const bold = (span.attributes & TextAttributes.BOLD) !== 0
  const italic = (span.attributes & TextAttributes.ITALIC) !== 0
  context.font = `${italic ? "italic " : ""}${bold ? "bold " : ""}${FONT_SIZE}px Menlo, monospace`
  context.fillStyle = cssColor(foreground, (span.attributes & TextAttributes.DIM) !== 0 ? 0.6 : 1)
  let textCol = col
  for (const { segment } of segmenter.segment(span.text)) {
    context.fillText(segment, PADDING + textCol * CELL_WIDTH, y + CELL_HEIGHT / 2)
    textCol += stringWidth(segment)
  }

  context.strokeStyle = context.fillStyle
  context.lineWidth = 1
  if ((span.attributes & TextAttributes.UNDERLINE) !== 0) {
    line(context, x, y + CELL_HEIGHT - 2, x + spanWidth, y + CELL_HEIGHT - 2)
  }
  if ((span.attributes & TextAttributes.STRIKETHROUGH) !== 0) {
    line(context, x, y + CELL_HEIGHT / 2, x + spanWidth, y + CELL_HEIGHT / 2)
  }
}

function cssColor(color: CapturedSpan["fg"], opacity = 1): string {
  const [red, green, blue, alpha] = color.toInts()
  return `rgba(${red}, ${green}, ${blue}, ${(alpha / 255) * opacity})`
}

function line(
  context: ReturnType<ReturnType<typeof createCanvas>["getContext"]>,
  fromX: number,
  fromY: number,
  toX: number,
  toY: number,
) {
  context.beginPath()
  context.moveTo(fromX, fromY)
  context.lineTo(toX, toY)
  context.stroke()
}

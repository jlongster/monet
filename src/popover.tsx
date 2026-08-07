import type { BoxRenderable } from "@opentui/core"
import { createPortal } from "@opentui/react"
import { createContext, type ReactNode, type RefObject, useContext, useState } from "react"

import { gray } from "./theme"

const PopoverTargetContext = createContext<BoxRenderable | null>(null)
const shadowOpacities = [0.68, 0.48, 0.32, 0.20, 0.12, 0.06]
const topShadowDepth = 1

export const popoverTop = (
  anchorTop: number,
  anchorHeight: number,
  containerHeight: number,
  popoverHeight: number,
) => {
  const below = anchorTop + anchorHeight
  return containerHeight - below < popoverHeight
    ? Math.max(0, anchorTop - popoverHeight)
    : below
}

export const popoverLeft = (
  anchorLeft: number,
  anchorWidth: number,
  containerWidth: number,
  popoverWidth: number,
  align: "left" | "right",
) => Math.max(
  0,
  Math.min(
    align === "right" ? anchorLeft + anchorWidth - popoverWidth : anchorLeft,
    containerWidth - popoverWidth,
  ),
)

export const overlayPopoverTop = (
  anchorTop: number,
  anchorHeight: number,
  containerHeight: number,
  popoverHeight: number,
) => containerHeight - anchorTop < popoverHeight
  ? Math.max(0, anchorTop + anchorHeight - popoverHeight)
  : anchorTop

export function PopoverProvider({
  children,
}: {
  children: ReactNode
}) {
  const [target, setTarget] = useState<BoxRenderable | null>(null)
  return (
    <PopoverTargetContext.Provider value={target}>
      <box style={{ width: "100%", height: "100%", position: "relative" }}>
        {children}
        <box
          ref={setTarget}
          style={{
            position: "absolute",
            top: 0,
            left: 0,
            width: 0,
            height: 0,
            zIndex: 1000,
          }}
          shouldFill={false}
        />
      </box>
    </PopoverTargetContext.Provider>
  )
}

export function Popover({
  open,
  anchorRef,
  height,
  width,
  align = "left",
  anchorMode = "adjacent",
  children,
}: {
  open: boolean
  anchorRef: RefObject<BoxRenderable | null>
  height: number
  width?: number
  align?: "left" | "right"
  anchorMode?: "adjacent" | "overlay"
  children: ReactNode
}) {
  const target = useContext(PopoverTargetContext)
  const anchor = anchorRef.current
  if (!open || !target || !anchor) return null
  const anchorTop = anchor.screenY - target.screenY
  const containerHeight = target.parent?.height ?? Number.POSITIVE_INFINITY
  const containerWidth = target.parent?.width ?? Number.POSITIVE_INFINITY
  const popoverWidth = width ?? anchor.width
  const top = anchorMode === "overlay"
    ? overlayPopoverTop(anchorTop, anchor.height, containerHeight, height)
    : popoverTop(anchorTop, anchor.height, containerHeight, height)
  const left = popoverLeft(
    anchor.screenX - target.screenX,
    anchor.width,
    containerWidth,
    popoverWidth,
    align,
  )

  return createPortal(
    <box
      style={{
        position: "absolute",
        top: top - topShadowDepth,
        left: left - shadowOpacities.length,
        width: popoverWidth + shadowOpacities.length * 2,
        height: height + topShadowDepth + shadowOpacities.length,
        zIndex: 1000,
      }}
    >
      {shadowOpacities.map((opacity, index) => {
        const distance = index + 1
        const topDistance = Math.min(distance, topShadowDepth)
        return (
          <box
            key={distance}
            style={{
              position: "absolute",
              top: topShadowDepth - topDistance,
              left: shadowOpacities.length - distance,
              width: popoverWidth + distance * 2,
              height: height + topDistance + distance,
              backgroundColor: gray[900],
              opacity,
              zIndex: shadowOpacities.length - distance,
            }}
          />
        )
      })}
      <box
        style={{
          position: "absolute",
          top: topShadowDepth,
          left: shadowOpacities.length,
          width: popoverWidth,
          height,
          zIndex: shadowOpacities.length + 1,
        }}
      >
        {children}
      </box>
    </box>,
    target,
    null,
    anchor.id,
  )
}

import type { BoxRenderable } from "@opentui/core"
import { createPortal } from "@opentui/react"
import { createContext, type ReactNode, type RefObject, useContext, useState } from "react"

const PopoverTargetContext = createContext<BoxRenderable | null>(null)

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
  children,
}: {
  open: boolean
  anchorRef: RefObject<BoxRenderable | null>
  height: number
  children: ReactNode
}) {
  const target = useContext(PopoverTargetContext)
  const anchor = anchorRef.current
  if (!open || !target || !anchor) return null

  return createPortal(
    <box
      buffered
      style={{
        position: "absolute",
        top: anchor.screenY - target.screenY + anchor.height,
        left: anchor.screenX - target.screenX,
        width: anchor.width,
        height,
        zIndex: 1000,
      }}
    >
      {children}
    </box>,
    target,
    null,
    anchor.id,
  )
}

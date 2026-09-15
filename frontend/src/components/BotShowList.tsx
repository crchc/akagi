import type { RefObject } from 'react'
import { Mahgen } from '@/components/Mahgen'
import { hexToRgba } from '@/lib/botShow'
import { mjaiToMahgen } from '@/lib/tileIdx'
import { cn } from '@/lib/utils'
import type { ShowItem } from '@/types'

type Props = {
  items: ShowItem[]
  containerRef?: RefObject<HTMLOListElement | null>
  className?: string
}

export function BotShowList({ items, containerRef, className }: Props) {
  return (
    <ol ref={containerRef} className={cn('flex flex-col gap-1', className)}>
      {items.map((it, i) => {
        const seq = it.tiles ?? (it.pais ? mjaiToMahgen(it.pais) : '')
        return (
          <li
            key={i}
            className={cn(
              'flex h-18 shrink-0 items-center gap-2 overflow-hidden rounded-md border border-border px-2',
            )}
            style={{
              backgroundColor: hexToRgba(it.color, 0.1),
              borderLeftColor: it.color,
              borderLeftWidth: it.color ? 3 : undefined,
            }}
          >
            {seq && (
              <Mahgen
                seq={seq}
                kind="bot-show"
                containerRef={containerRef}
              />
            )}
            <div className="flex min-w-0 flex-1 flex-col">
              {it.label && (
                <span className="truncate text-sm text-foreground">{it.label}</span>
              )}
              {it.note && (
                <span className="truncate text-[10px] text-muted-foreground">{it.note}</span>
              )}
            </div>
            {it.value && (
              <span
                className={cn(
                  'font-mono tabular-nums text-foreground/90',
                  'text-xs',
                )}
              >
                {it.value}
              </span>
            )}
          </li>
        )
      })}
    </ol>
  )
}

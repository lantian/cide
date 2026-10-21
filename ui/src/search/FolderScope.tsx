import { useEffect, useId, useRef, useState } from 'react'
import { TextInput } from '@/kit/components/Field'
import { PickerList, PickerRow } from '@/kit/components/Overlay'
import { Icon } from '@/icons/Icon'
import { searchSessions } from '@/ipc/client'
import styles from './Search.module.css'

/** Scope completion is directory-only and reads the full index, not Explorer's visible rows. */
export function FolderScope({ project, value, onChange, disabled, invalid }: {
  project: string | null; value: string; onChange: (value: string) => void; disabled: boolean; invalid: boolean;
}) {
  const id = useId()
  const popup = useRef<HTMLDivElement>(null)
  const [focused, setFocused] = useState(false)
  const [dismissed, setDismissed] = useState(false)
  const [options, setOptions] = useState<string[]>([])
  const [selected, setSelected] = useState(0)
  useEffect(() => {
    let alive = true
    setOptions([]); setSelected(0)
    if (!focused || disabled || !project || dismissed) return
    const timer = setTimeout(() => {
      void searchSessions.suggestScopes(project, value).then(rows => { if (alive) setOptions(rows) }).catch(() => { if (alive) setOptions([]) })
    }, 120)
    return () => { alive = false; clearTimeout(timer) }
  }, [project, value, focused, disabled, dismissed])
  useEffect(() => { popup.current?.querySelector('[aria-selected="true"]')?.scrollIntoView({ block: 'nearest' }) }, [selected])
  const open = focused && !dismissed && options.length > 0
  const accept = (option: string) => { onChange(option); setDismissed(true) }
  return <div className={styles.folderScope}>
    <TextInput size="sm" value={value} aria-label="Folder to search" placeholder="Folder (all)" disabled={disabled} invalid={invalid}
      role="combobox" aria-autocomplete="list" aria-expanded={open} aria-controls={open ? id : undefined}
      aria-activedescendant={open ? `${id}-${selected}` : undefined} autoComplete="off" spellCheck={false}
      onFocus={() => { setFocused(true); setDismissed(false) }} onBlur={() => setFocused(false)}
      onChange={e => { setDismissed(false); onChange(e.target.value) }}
      onKeyDown={e => {
        if (e.nativeEvent.isComposing) return
        if (open && ['ArrowDown', 'ArrowUp', 'Enter', 'Tab', 'Escape'].includes(e.key)) {
          e.preventDefault(); e.stopPropagation()
          if (e.key === 'Escape') setDismissed(true)
          else if (e.key === 'ArrowDown' || e.key === 'ArrowUp') setSelected(n => Math.max(0, Math.min(options.length - 1, n + (e.key === 'ArrowDown' ? 1 : -1))))
          else if (options[selected]) accept(options[selected])
        } else if (e.key === 'ArrowDown') { e.preventDefault(); setDismissed(false) }
      }} />
    {open && <div ref={popup} className={styles.folderPopup}>
      <PickerList id={id} aria-label="Project folders">
        {options.map((option, index) => <PickerRow key={option} id={`${id}-${index}`} selected={index === selected}
          onMouseDown={e => { e.preventDefault(); accept(option) }} title={option}>
          <Icon name="folder" size={1} /><span className={styles.resultText}>{option}</span>
        </PickerRow>)}
      </PickerList>
    </div>}
  </div>
}

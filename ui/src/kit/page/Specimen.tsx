/**
 * The kit page's frames: a chapter (one family of components), a specimen (one component, with
 * when to use it, a live stage and its spec), and small layout helpers for the stage.
 */
import type { ReactElement, ReactNode } from 'react'

import styles from './kit.module.css'

export function Chapter({
  id,
  title,
  lead,
  children,
}: {
  id: string
  title: string
  lead: ReactNode
  children: ReactNode
}): ReactElement {
  return (
    <section id={id} className={styles.chapter}>
      <h2 className={styles.chapterTitle}>{title}</h2>
      <p className={styles.chapterLead}>{lead}</p>
      {children}
    </section>
  )
}

export type Spec = ReadonlyArray<readonly [string, string]>

export function Specimen({
  name,
  source,
  use,
  ground = 'bg',
  flush = false,
  spec,
  children,
}: {
  name: string
  /** The kit component that draws it: `Button.tsx › Button`. */
  source: string
  /** When to reach for it, and when not to. */
  use: ReactNode
  ground?: 'bg' | 'panel' | 'chrome' | undefined
  flush?: boolean | undefined
  /** Property → value rows: the numbers a reimplementation must hit. */
  spec?: Spec | undefined
  children: ReactNode
}): ReactElement {
  return (
    <article className={styles.specimen}>
      <header className={styles.specimenHead}>
        <h3 className={styles.specimenName}>{name}</h3>
        <span className={styles.specimenSource}>{source}</span>
      </header>
      <div className={styles.specimenUse}>{use}</div>
      <div className={styles.stage} data-ground={ground} data-flush={flush || undefined}>
        {children}
      </div>
      {spec !== undefined && (
        <table className={styles.spec}>
          <thead>
            <tr>
              <th>Property</th>
              <th>Value</th>
            </tr>
          </thead>
          <tbody>
            {spec.map(([k, v]) => (
              <tr key={k}>
                <td>{k}</td>
                <td>
                  <code>{v}</code>
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      )}
    </article>
  )
}

export function Stack({ children }: { children: ReactNode }): ReactElement {
  return <div className={styles.stack}>{children}</div>
}

export function Line({ children }: { children: ReactNode }): ReactElement {
  return <div className={styles.row}>{children}</div>
}

/** One labelled example on a stage: `state: hover` above the thing. */
export function Cell({
  label,
  grow = false,
  children,
}: {
  label: string
  grow?: boolean | undefined
  children: ReactNode
}): ReactElement {
  return (
    <div className={grow ? `${styles.cell} ${styles.grow}` : styles.cell}>
      <span className={styles.cellLabel}>{label}</span>
      {children}
    </div>
  )
}

/** A mock sidebar panel or pane for a composed example. */
export function Frame({
  wide = false,
  children,
}: {
  wide?: boolean | undefined
  children: ReactNode
}): ReactElement {
  return <div className={wide ? `${styles.frame} ${styles.frameWide}` : styles.frame}>{children}</div>
}

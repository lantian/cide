/**
 * Joins class names, dropping the falsy ones. The kit's own copy rather than an import from a
 * panel (`sidebar/AgentsPanel/RunRow.tsx` and `TaskDetail.tsx` each have one): a kit component
 * must not pull a panel's module graph into a page that has no app behind it.
 */
export function cx(...parts: Array<string | undefined | false | null>): string {
  return parts.filter(Boolean).join(' ')
}

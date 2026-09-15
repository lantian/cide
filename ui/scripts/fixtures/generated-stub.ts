/**
 * A stand-in for `@/ipc/generated`, for the checks that compile a seam module standalone.
 *
 * `sidebar/DockerPanel/adapt.ts` is the only file `check:docker` compiles that imports anything
 * from outside its own directory, and what it imports is **types only**. The check drives its
 * runtime behaviour — what it does when handed a payload that is not what those types promise —
 * so the real declarations would add nothing but a resolution problem.
 *
 * `any` is deliberate and is the point: every assertion this stub enables is about a value the
 * type system said could not exist. Narrowing it here would make those cases unreachable, which
 * is exactly the blind spot the wire keeps walking into.
 */
export type DockerBoard = any

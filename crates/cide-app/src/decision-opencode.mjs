// Bundled OpenCode 2 plugin. No packages, storage, transcript rewriting or permission overrides.
export default {
  id: 'cide.decision',
  async setup(ctx) {
    const endpoint = process.env.CIDE_DECISION_ENDPOINT
    const token = process.env.CIDE_DECISION_TOKEN
    if (!endpoint || !token) return
    const abort = new AbortController()
    const registrations = []
    const pending = new Map()
    const epochs = new Map()
    const active = new Set()
    const switchingModel = new Map()
    const totals = new Map()
    const send = async (body) => {
      try {
        const response = await fetch(endpoint, {
          method: 'POST', headers: { Authorization: `Bearer ${token}`, 'Content-Type': 'application/json' },
          // A user prompt can evaluate research and a permitted model profile in
          // sequence. Each inference retains the broker's own <= 30 s deadline.
          body: JSON.stringify(body), signal: AbortSignal.any([abort.signal, AbortSignal.timeout(body.event?.kind === 'prompt' ? 61000 : 31000)]),
        })
        return response.ok ? await response.json() : {}
      } catch { return {} }
    }
    const valid = async (sessionID, epoch, action) => action.id && epochs.get(sessionID) === epoch &&
      (await send({ op: 'valid', native: sessionID, id: action.id, generation: action.generation })).valid
    const ack = (action, ok) => send({ op: 'ack', native: action.native, id: action.id, ok })
    const event = async (sessionID, data) => ({ ...await send({ op: 'event', native: sessionID, event: data }), native: sessionID })
    registrations.push(await ctx.session.hook('prompt', async (e) => {
      const epoch = (epochs.get(e.sessionID) ?? 0) + 1
      epochs.set(e.sessionID, epoch); active.add(e.sessionID); pending.delete(e.sessionID)
      const action = await event(e.sessionID, { kind: 'prompt', id: e.messageID ?? '', text: e.prompt.text ?? '' })
      if (action.research?.text) {
        const research = { ...action.research, native: e.sessionID }
        if (await valid(e.sessionID, epoch, research)) pending.set(e.sessionID, research)
      }
      if (!action.profile || !await valid(e.sessionID, epoch, action)) return
      const slash = action.profile.model.indexOf('/')
      if (slash < 1) { await ack(action, false); return }
      try {
        switchingModel.set(e.sessionID, action.profile.model)
        await ctx.session.switchModel({ sessionID: e.sessionID, model: {
          providerID: action.profile.model.slice(0, slash), id: action.profile.model.slice(slash + 1),
          ...(action.profile.effort ? { variant: action.profile.effort } : {}),
        } })
        await ack(action, true)
      } catch { switchingModel.delete(e.sessionID); await ack(action, false) }
    }))
    registrations.push(await ctx.tool.hook('execute.after', async (e) => {
      const epoch = epochs.get(e.sessionID)
      if (e.tool === 'todowrite' && e.status === 'completed' && Array.isArray(e.input?.todos)) {
        await event(e.sessionID, {kind:'plan',obligations:e.input.todos.filter(t=>t.status!=='completed'&&t.status!=='cancelled').map(t=>t.content)})
      }
      const action = await event(e.sessionID, {
        kind: 'tool', id: e.id ?? e.callID ?? '', tool: e.tool, command: e.input?.command ?? '',
        text: JSON.stringify({ input: e.input, result: e.result, error: e.error }).slice(0, 4096),
        failed: e.status === 'error', changed: e.status === 'completed' && /^(edit|write|apply_patch|patch)$/.test(e.tool),
      })
      if (action.text && await valid(e.sessionID, epoch, action)) pending.set(e.sessionID, action)
    }))
    // Add the decision to the next request's system context, without changing tool results,
    // user text, tool history or durable instructions.
    registrations.push(await ctx.session.hook('context', async (e) => {
      const action = pending.get(e.sessionID); pending.delete(e.sessionID)
      if (action && await valid(e.sessionID, epochs.get(e.sessionID), action)) {
        e.system.push({ type: 'text', text: `Cide decision assistance: ${action.text}` })
        await ack(action, true)
      }
    }))
    void (async () => {
      try {
        for await (const raw of ctx.event.subscribe({ signal: abort.signal })) {
          const p = raw.data ?? raw.properties ?? raw
          const sessionID = p.sessionID ?? p.info?.sessionID
          if (!sessionID) continue
          if (raw.type === 'session.model.selected') {
            const expected = switchingModel.get(sessionID)
            switchingModel.delete(sessionID)
            // A manual selection can precede the first prompt. Initial default-model
            // assignment has no previous model and must not silently disable Auto.
            if ((!expected || expected !== `${p.model?.providerID}/${p.model?.id}`) && (epochs.has(sessionID) || p.previous)) {
              await send({op:'pin',native:sessionID})
            }
          }
          if (!epochs.has(sessionID)) continue
          if (/permission.*asked|question.*asked|form.created|session.execution.interrupted|session.execution.failed|session.error/.test(raw.type)) {
            epochs.set(sessionID, (epochs.get(sessionID) ?? 0) + 1)
            pending.delete(sessionID)
            if (raw.type.startsWith('session.')) active.delete(sessionID)
            await event(sessionID, { kind: raw.type.startsWith('session.') ? 'stop' : 'waiting' })
          }
          if (/permission.*replied|question.*replied|form.replied|form.cancelled/.test(raw.type)) await event(sessionID, { kind: 'resumed' })
          if (raw.type === 'session.text.ended') {
            await event(sessionID,{kind:'message',id:raw.id??'',text:(p.text??'').slice(0,4096)})
          }
          if (raw.type === 'session.step.ended') {
            const t=totals.get(sessionID) ?? {seen:new Set()}
            const id=raw.id ?? p.assistantMessageID
            if (id && !t.seen.has(id)) {
              t.seen.add(id); if(t.seen.size>2048)t.seen.delete(t.seen.values().next().value)
              for (const [key,value] of Object.entries({inputTokens:p.tokens?.input,outputTokens:p.tokens?.output,cacheTokens:p.tokens?.cache?.read,cost:p.cost})) {
                if (typeof value==='number' && Number.isFinite(value) && value>=0) t[key]=(t[key]??0)+value
              }
              totals.set(sessionID,t);await event(sessionID,{kind:'usage',inputTokens:t.inputTokens,outputTokens:t.outputTokens,cacheTokens:t.cacheTokens,cost:t.cost})
            }
          }
          const idle = raw.type === 'session.execution.succeeded' || raw.type === 'session.idle' || raw.type === 'session.status' && p.status?.type === 'idle'
          if (idle && active.delete(sessionID)) {
            const epoch = epochs.get(sessionID)
            const action = await event(sessionID, { kind: 'complete', id: raw.id ?? String(epoch) })
            if (action.continuation && await valid(sessionID, epoch, action)) {
              try {
                // Synthetic admission bypasses the user-prompt hook, preserving the request's
                // continuation budget and never resetting a user cancellation.
                await ctx.session.synthetic({ sessionID, text: action.text, resume: true })
                active.add(sessionID); await ack(action, true)
              } catch { await ack(action, false) }
            }
          }
        }
      } catch { /* Unavailable native stream: optimisation stays inactive. */ }
    })()
    return async () => { abort.abort(); pending.clear(); await Promise.allSettled(registrations.map(r => r.dispose())) }
  },
}

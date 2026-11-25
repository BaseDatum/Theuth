// 216 §10.2, contract §12.6: the screen of a `plugin` item. An install / widen / code-changed card (`plugin:<name>:…`):
// who asks (the "plugin · <name>" attribution), the card's plain-words lines with their risks, the changed files, the
// folder, and Approve · Review · Snooze · Dismiss in the bar. The full card (and its hashes) comes from
// `plugin_get(name)`, never from the item. A plugin's notice (`plugin-notify:…`): its title and body, and the way to it.
import { useQuery } from '@tanstack/react-query'
import type { ReactNode } from 'react'
import { GrantLines, shown } from '@/components/plugins/GrantLines'
import { PluginIcon } from '@/components/plugins/icons'
import { errorText } from '@/components/settings/shared'
import { queryKeys, useApi } from '@/lib/rpc'
import type { CardKind } from '@/rpc/bindings/method.plugins.output'
import type { AttentionItem } from '@/rpc/bindings/method.roster.output'
import { KindActions } from './actions'
import { pluginRef } from './labels'
import { Body, SectionTitle, useResolved } from './shared'
import { ActionBar, Source } from './source'

/** What the card asks, under its title. */
export const CARD_LEAD: Record<CardKind, string> = {
  install: 'Approving lets it run on this machine as you, with what is listed below. Nothing of it runs before you do.',
  widen: 'It asks for more than you approved. Until you approve, it keeps what it had and the new lines stay off.',
  code_changed: 'Its code changed since you approved it. Until you approve, the old process keeps running and nothing new starts.',
}

function Fact({ label, children }: { label: string; children: ReactNode }) {
  return (
    <div className="flex flex-col gap-0.5">
      <SectionTitle>{label}</SectionTitle>
      <div className="text-sm">{children}</div>
    </div>
  )
}

/** "plugin · <name>": who raised the item, drawn by the app, never by the plugin. */
function Attribution({ name, icon }: { name: string; icon: string }) {
  return (
    <p className="flex items-center gap-1.5 text-xs text-fg-tertiary">
      <PluginIcon icon={icon} className="size-3.5 shrink-0" />
      plugin · <span className="text-fg-secondary">{name}</span>
    </p>
  )
}

export function PluginInstallScreen({ item }: { item: AttentionItem }) {
  const onResolved = useResolved(item)
  const ref = pluginRef(item)
  return (
    <>
      <Body>
        <Source item={item} />
        {ref && ref.notify ? <Notice item={item} name={ref.name} /> : <Card item={item} name={ref?.name ?? null} />}
      </Body>
      <ActionBar>
        <KindActions item={item} size="default" onResolved={onResolved} />
      </ActionBar>
    </>
  )
}

function Notice({ item, name }: { item: AttentionItem; name: string }) {
  return (
    <>
      <div className="flex flex-col gap-1.5">
        <Attribution name={name} icon="puzzle" />
        <h1 className="text-xl font-semibold leading-snug md:text-2xl">{shown(item.title)}</h1>
      </div>
      {item.detail && <p className="text-sm whitespace-pre-wrap text-fg-secondary">{shown(item.detail)}</p>}
    </>
  )
}

function Card({ item, name }: { item: AttentionItem; name: string | null }) {
  const api = useApi()
  const detail = useQuery({ enabled: name !== null, queryKey: queryKeys.plugin(name ?? ''), queryFn: () => api.plugin_get(name as string) })
  const card = detail.data?.card ?? null
  // The item outlives its card when the plugin changed again or was approved elsewhere: say so, keep Dismiss.
  const stale = detail.data !== undefined && card?.id !== item.id
  const icon = detail.data?.info.placements[0]?.icon ?? 'puzzle'
  return (
    <>
      <div className="flex flex-col gap-1.5">
        {name && <Attribution name={name} icon={icon} />}
        <h1 className="text-xl font-semibold leading-snug md:text-2xl">{shown(item.title)}</h1>
        {card && !stale && <p className="text-sm text-fg-secondary">{CARD_LEAD[card.kind]}</p>}
      </div>
      {detail.error && <p className="text-sm break-words text-danger">{errorText(detail.error)}</p>}
      {detail.isPending && name !== null && <p className="text-sm text-fg-tertiary">Loading the card…</p>}
      {stale && <p className="text-sm text-fg-secondary">{card ? 'This card was replaced by a newer one; review the plugin in Settings.' : 'Nothing waits for approval any more.'}</p>}
      {card && !stale && (
        <>
          {card.unverified && (
            <p className="rounded-md border border-warning-border bg-warning-bg px-3 py-2 text-sm text-warning">
              The saved approval for {card.plugin} could not be verified, so it was not started. Approve only if you expect this plugin here.
            </p>
          )}
          <div className="flex flex-col gap-1">
            <SectionTitle>{card.kind === 'install' ? 'It may' : 'New since your approval'}</SectionTitle>
            <GrantLines lines={card.lines} />
          </div>
          {card.changed_paths.length > 0 && (
            <Fact label="Changed files">
              <ul className="flex flex-col gap-0.5 font-mono text-xs">
                {card.changed_paths.map((p) => (
                  <li key={p} className="break-all">
                    {shown(p)}
                  </li>
                ))}
              </ul>
            </Fact>
          )}
          <div className="grid gap-4 sm:grid-cols-2">
            <Fact label="Folder">
              <span className="break-all font-mono text-xs">{shown(card.dir)}</span>
            </Fact>
            {card.version && <Fact label="Version">{shown(card.version)}</Fact>}
          </div>
        </>
      )}
    </>
  )
}

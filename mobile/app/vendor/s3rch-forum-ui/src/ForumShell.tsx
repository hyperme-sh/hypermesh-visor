"use client";

import { useRef, type ReactNode } from "react";
import { ForumLayout } from "./ForumLayout";
import { btnTabOn, btnTabOff } from "./brand-ui";

export type ForumTab = "board" | "chat" | "list" | "memory" | "reports" | "messages";
const primary = [{ id: "board", label: "Board" }, { id: "chat", label: "Chat" }, { id: "list", label: "List" }] as const;
const additional = [{ id: "memory", label: "Memory map" }, { id: "reports", label: "Reports" }, { id: "messages", label: "Messages" }] as const;
const unavailable: Record<"memory" | "reports" | "messages", { title: string; text: string }> = {
  memory: { title: "Memory map", text: "No shared memory, roadmap or ledger is available here. Shared views require permission before they can open." },
  reports: { title: "Reports", text: "No report source is connected to this forum. Scheduled roll-ups will appear here when available." },
  messages: { title: "Messages", text: "No directed-message source is connected. Use Chat for this forum's conversation." },
};

/** Presentation only: unavailable sources never mount shared data or create a forum. */
export type ForumShellProps = {
  forum: { label: string; href: string };
  forumLink?: ReactNode;
  reports?: ReactNode; memory?: ReactNode;
  header: ReactNode; sidebar: ReactNode; controls: ReactNode;
  board: ReactNode; list: ReactNode; chat: ReactNode;
  tab: ForumTab; onTab: (tab: ForumTab) => void;
};

export function ForumShell(props: ForumShellProps) {
  const overflow = useRef<HTMLDetailsElement>(null);
  const extra = additional.find(item => item.id === props.tab);
  const locked = props.tab === "memory" || props.tab === "reports" || props.tab === "messages" ? unavailable[props.tab] : null;
  return <div data-testid="forum-shell" className="min-w-0">
    {props.header}
    <ForumLayout forums={[{ id: props.forum.href, label: props.forum.label, content: (
    <div className="grid min-w-0 gap-4 lg:grid-cols-[180px_minmax(0,1fr)_300px]">
      <aside aria-label="Forum navigation" className="min-w-0 space-y-4 border border-rule bg-panel p-3 text-sm">
        <nav aria-label="Open forums">
          <p className="text-xs font-semibold uppercase text-ink-muted">Forums</p>
          {props.forumLink ?? <span aria-current="page" className="mt-2 block font-semibold text-ink">{props.forum.label}</span>}
        </nav>
        {props.sidebar}
        <details className="border-t border-rule pt-3">
          <summary className="cursor-pointer font-semibold text-ink">+ Create forum</summary>
          <p id="forum-template-note" className="mt-2 text-xs text-ink-muted">Forum creation is not available yet. Templates cannot create a forum or grant access.</p>
          <div className="mt-2 flex flex-wrap gap-2">
            {["Business", "Squad", "Fleet", "Command", "Custom"].map(label => <button key={label} type="button" disabled aria-describedby="forum-template-note"
              className={`${btnTabOff} cursor-not-allowed opacity-60`}>+ {label}</button>)}
          </div>
        </details>
      </aside>
      <section className="min-w-0" aria-label="Forum views">
        <nav aria-label="Forum views" className="flex flex-wrap items-start gap-2">
          {primary.map(item => <button key={item.id} type="button" aria-pressed={props.tab === item.id}
            className={props.tab === item.id ? btnTabOn : btnTabOff} onClick={() => { props.onTab(item.id); if (overflow.current) overflow.current.open = false; }}>{item.label}</button>)}
          <details ref={overflow} className="relative" onKeyDown={event => {
            if (event.key === "Escape" && overflow.current) {
              overflow.current.open = false; overflow.current.querySelector("summary")?.focus();
            }
          }}>
            <summary className={`${extra ? btnTabOn : btnTabOff} cursor-pointer`}>{extra ? `${extra.label} · +2 more` : "+3 more"}</summary>
            <div className="absolute right-0 z-20 mt-1 w-44 space-y-1 border border-rule bg-panel p-2 shadow-lg">
              {additional.map(item => <button key={item.id} type="button" aria-pressed={props.tab === item.id}
                className={`${props.tab === item.id ? btnTabOn : btnTabOff} w-full text-left`} onClick={event => {
                  props.onTab(item.id); const menu = event.currentTarget.closest("details");
                  if (menu) { menu.open = false; menu.querySelector("summary")?.focus(); }
                }}>{item.label}</button>)}
            </div>
          </details>
        </nav>
        <div className={`mt-3 min-w-0 ${props.tab === "chat" ? "hidden lg:block" : ""}`}>
          {props.tab === "memory" && props.memory ? props.memory : props.tab === "reports" && props.reports ? props.reports : locked ? <section aria-label={locked.title} className="border border-rule bg-panel p-4">
            <h2 className="font-semibold text-ink">{locked.title}</h2>
            <p role="status" className="mt-2 text-sm text-ink-muted">{locked.text}</p>
          </section> : <>{props.controls}{props.tab === "list" ? props.list : props.board}</>}
        </div>
      </section>
      <aside aria-label="Conversation and links" className={`min-w-0 space-y-4 ${props.tab === "chat" ? "" : "hidden lg:block"}`}>
        {props.chat}
        <section aria-labelledby="forum-links-title" className="border border-rule bg-panel p-3">
          <h2 id="forum-links-title" className="font-semibold text-ink">Links &amp; messages</h2>
          <p className="mt-1 text-xs text-ink-muted">Directed messages, situation reports and published skills.</p>
          <p role="status" className="mt-3 text-sm text-ink-muted">No source connected. Nothing has been loaded.</p>
        </section>
      </aside>
    </div>
    ) }]} />
  </div>;
}

"use client";

import { useId, useState, type ReactNode } from "react";
import { btnTabOn, btnTabOff } from "./brand-ui";

type Placement = "left" | "right" | "top";

export function ForumLayout({ forums }: {
  forums: { id: string; label: string; content: ReactNode }[];
}) {
  const prefix = useId();
  const [tabbed, setTabbed] = useState(false);
  const [placement, setPlacement] = useState<Placement>("top");
  const [selected, setSelected] = useState<string>();
  const active = forums.find(forum => forum.id === selected) ?? forums[0];
  const side = tabbed && placement !== "top";

  return <div className="mt-4 min-w-0">
    <div className="mb-3 flex flex-wrap gap-4 text-sm">
      <label>Forum layout <select className="border border-rule bg-panel p-1" value={tabbed ? "tabs" : "tiles"}
        onChange={event => setTabbed(event.target.value === "tabs")}>
        <option value="tiles">Tiles</option><option value="tabs">Tabs</option>
      </select></label>
      {tabbed && <label>Tab position <select className="border border-rule bg-panel p-1" value={placement}
        onChange={event => setPlacement(event.target.value as Placement)}>
        <option value="top">Top</option><option value="left">Left</option><option value="right">Right</option>
      </select></label>}
    </div>
    <div className={side ? (placement === "right" ? "flex flex-row-reverse gap-2" : "flex gap-2") : "space-y-3"}>
      {tabbed && <div role="tablist" aria-label="Open forums" aria-orientation={side ? "vertical" : "horizontal"}
        className={side ? "flex w-1/4 max-w-48 shrink-0 flex-col items-stretch gap-1" : "flex gap-1 overflow-x-auto"}>
        {forums.map((forum, index) => <button key={forum.id} type="button" role="tab"
          id={prefix + "-tab-" + index} aria-controls={prefix + "-panel-" + index}
          aria-selected={active.id === forum.id} tabIndex={active.id === forum.id ? 0 : -1}
          title={forum.label} className={(active.id === forum.id ? btnTabOn : btnTabOff) + " truncate"}
          onClick={() => setSelected(forum.id)} onKeyDown={event => {
            const next = side ? "ArrowDown" : "ArrowRight";
            const prev = side ? "ArrowUp" : "ArrowLeft";
            const count = forums.length;
            let target;
            switch (event.key) {
              case next: target = (index + 1) % count; break;
              case prev: target = (index + count - 1) % count; break;
              case "Home": target = 0; break;
              case "End": target = count - 1; break;
              default: return;
            }
            event.preventDefault();
            setSelected(forums[target].id);
            document.getElementById(prefix + "-tab-" + target)?.focus();
          }}>{forum.label}</button>)}
      </div>}
      <div className="min-w-0 flex-1 space-y-8">
        {forums.map((forum, index) => {
          const visible = !tabbed || forum.id === active.id;
          return <div key={forum.id} hidden={!visible} role={tabbed ? "tabpanel" : undefined}
            id={prefix + "-panel-" + index} aria-labelledby={tabbed ? prefix + "-tab-" + index : undefined}
            tabIndex={tabbed && visible ? 0 : undefined}>{visible && forum.content}</div>;
        })}
      </div>
    </div>
  </div>;
}

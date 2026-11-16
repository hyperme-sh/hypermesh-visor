"use client";

import type { ChangeEvent } from "react";
import { btnPrimary, field } from "./brand-ui";

export type MessageComposerProps = {
  value: string;
  onChange: (value: string) => void;
  onSend: () => void;
  disabled?: boolean;
  busy?: boolean;
  maxLength: number;
  rows?: number;
  placeholder?: string;
  sendLabel?: string;
  className?: string;
  fieldClassName?: string;
  buttonClassName?: string;
};

/** Controlled presentation only. The authority-owning caller accepts a send and
 * clears its draft; this component never stores credentials, text or transport. */
export function MessageComposer(props: MessageComposerProps) {
  const disabled = props.disabled || props.busy;
  const common = {
    value: props.value, disabled, maxLength: props.maxLength, placeholder: props.placeholder,
    className: props.fieldClassName ?? `mt-1 w-full ${field}`,
    onChange: (event: ChangeEvent<HTMLInputElement | HTMLTextAreaElement>) => props.onChange(event.target.value),
  };
  return <form className={props.className ?? "mt-4"} onSubmit={event => {
    event.preventDefault();
    if (!disabled && props.value.trim()) props.onSend();
  }}>
    <label className="block text-sm text-ink">Message
      {props.rows ? <textarea {...common} rows={props.rows}/> : <input {...common} type="text"/>}
    </label>
    <div className="mt-3"><button type="submit" disabled={disabled || !props.value.trim()}
      className={props.buttonClassName ?? btnPrimary}>{props.sendLabel ?? "Send"}</button></div>
  </form>;
}

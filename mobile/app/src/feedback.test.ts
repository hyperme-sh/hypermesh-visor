import { describe, expect, it } from 'vitest';
import { emptyDraft, feedbackDraft } from './feedback';
function draft() {
  let state = feedbackDraft(emptyDraft(), { type: 'consent', value: true });
  state = feedbackDraft(state, { type: 'edit', field: 'title', value: '  Example problem  ' });
  return feedbackDraft(state, { type: 'edit', field: 'description', value: '  My description\nSecond line  ' });
}
describe('memory-only feedback preparation', () => {
  it('starts off and cannot retain text without opt-in', () => {
    const initial = emptyDraft();
    expect(initial.optedIn).toBe(false);
    expect(feedbackDraft(initial, { type: 'edit', field: 'title', value: 'private' })).toEqual(initial);
    expect(feedbackDraft(initial, { type: 'preview' })).toEqual(initial);
  });
  it('previews only the manually entered fields', () => {
    const state = feedbackDraft(draft(), { type: 'preview' });
    expect(state.preview).toEqual({ kind: 'bug', title: 'Example problem', description: 'My description\nSecond line' });
  });
  it('invalidates approval preview after edits or category changes', () => {
    const state = feedbackDraft(draft(), { type: 'preview' });
    expect(feedbackDraft(state, { type: 'edit', field: 'description', value: 'changed' }).preview).toBeNull();
    expect(feedbackDraft(state, { type: 'kind', value: 'idea' }).preview).toBeNull();
    expect(state.preview).not.toBeNull();
  });
  it('withdrawal and suspension clear all text and preview immediately', () => {
    const state = feedbackDraft(draft(), { type: 'preview' });
    expect(feedbackDraft(state, { type: 'consent', value: false })).toEqual(emptyDraft());
    expect(feedbackDraft(state, { type: 'clear' })).toEqual(emptyDraft());
  });
  it('refuses blank or oversized fields without retaining rejected input', () => {
    expect(feedbackDraft({ ...draft(), title: '  ' }, { type: 'preview' }).preview).toBeNull();
    for (const field of ['title', 'description'] as const) {
      const state = feedbackDraft(draft(), { type: 'preview' });
      const next = feedbackDraft(state, { type: 'edit', field, value: 'x'.repeat(2001) });
      expect(next[field]).toBe(state[field]); expect(next.preview).toBeNull(); expect(next.error).not.toBe('');
    }
  });
  it('a new screen starts empty and does not reuse another draft', () => {
    const prior = draft();
    expect(emptyDraft().title).toBe(''); expect(emptyDraft().optedIn).toBe(false);
    expect(prior.title).not.toBe('');
  });
});

it('explains intake minimum lengths before native preparation', () => {
  const base = draft();
  let state=feedbackDraft(base,{type:'edit',field:'title',value:'abc'});
  expect(feedbackDraft(state,{type:'preview'}).error).toContain('at least 4');
  state=feedbackDraft(base,{type:'edit',field:'description',value:'123456789'});
  expect(feedbackDraft(state,{type:'preview'}).error).toContain('at least 10');
  state=feedbackDraft(base,{type:'edit',field:'title',value:'😀😀'});
  expect(feedbackDraft(state,{type:'preview'}).preview).toBeNull();
});

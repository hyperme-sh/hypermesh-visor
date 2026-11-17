export type Kind = 'bug' | 'idea';
export interface Preview { kind: Kind; title: string; description: string }
export interface FeedbackDraft {
  optedIn: boolean; kind: Kind; title: string; description: string;
  preview: Preview | null; error: string;
}
export type FeedbackAction = { type: 'consent'; value: boolean }
  | { type: 'edit'; field: 'title' | 'description'; value: string }
  | { type: 'kind'; value: Kind } | { type: 'preview' } | { type: 'clear' };
export function emptyDraft(): FeedbackDraft {
  return { optedIn: false, kind: 'bug', title: '', description: '', preview: null, error: '' };
}
const bytes = (value: string) => new TextEncoder().encode(value).length;
/** Memory-only draft. There is deliberately no credential or transport input. */
export function feedbackDraft(state: FeedbackDraft, action: FeedbackAction): FeedbackDraft {
  if (action.type === 'clear' || (action.type === 'consent' && !action.value)) return emptyDraft();
  if (action.type === 'consent') return { ...state, optedIn: true, error: '' };
  if (!state.optedIn) return state;
  if (action.type === 'edit') {
    const max = action.field === 'title' ? 140 : 2000;
    if (action.value.length > max || bytes(action.value) > max * 4) {
      return { ...state, preview: null, error: `Keep ${action.field === 'title' ? 'the title' : 'the description'} within ${max} characters.` };
    }
    return { ...state, [action.field]: action.value, preview: null, error: '' };
  }
  if (action.type === 'kind') return { ...state, kind: action.value, preview: null, error: '' };
  const title = state.title.trim();
  const description = state.description.trim();
  if (!title || !description) return { ...state, preview: null, error: 'Add a title and description before previewing.' };
  return { ...state, preview: { kind: state.kind, title, description }, error: '' };
}

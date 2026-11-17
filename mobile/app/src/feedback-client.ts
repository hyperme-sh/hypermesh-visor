import type { FeedbackPreview, VisorFeedbackNative } from './native';
import type { Preview } from './feedback';

export class FeedbackSendError extends Error { constructor(readonly retryable: boolean) { super('Report submission not confirmed.'); } }

/** Only public handles and manually entered text cross the native boundary. */
export class FeedbackClient {
  private generation = 0;
  private session: string | null = null;
  private confirmed: FeedbackPreview | null = null;
  constructor(private readonly native: () => VisorFeedbackNative) {}
  cancel() {
    this.generation++;
    const id = this.session;
    this.session = null; this.confirmed = null;
    if (id) void this.native().feedbackCancel({ sessionId: id }).catch(() => {});
  }
  private async start() {
    this.cancel();
    const generation = this.generation;
    const { sessionId } = await this.native().feedbackStart({ optedIn: true });
    if (generation !== this.generation) {
      await this.native().feedbackCancel({ sessionId }).catch(() => {});
      throw new Error('Feedback action cancelled.');
    }
    this.session = sessionId;
    return { sessionId, generation };
  }
  async consent(enabled: boolean) {
    const { sessionId, generation } = await this.start();
    await this.native().feedbackSetConsent({ sessionId, enabled });
    if (generation !== this.generation) throw new Error('Feedback action cancelled.');
    if (!enabled) this.cancel();
  }
  async prepare(preview: Preview) {
    const { sessionId, generation } = await this.start();
    const prepared = await this.native().feedbackPrepare({ sessionId, ...preview });
    if (generation !== this.generation) throw new Error('Feedback action cancelled.');
    if (!prepared.operationId || prepared.kind !== preview.kind || prepared.title !== preview.title || prepared.description !== preview.description) {
      this.cancel(); throw new Error('Feedback preview differs.');
    }
    this.confirmed = prepared;
    return prepared;
  }
  async submit() {
    const sessionId = this.session, report = this.confirmed, generation = this.generation;
    if (!sessionId || !report) throw new Error('Preview the report again.');
    // Native retains immutable bytes and idempotency after an uncertain result.
    let result: { submitted: boolean; retryable: boolean };
    try { result = await this.native().feedbackSubmit({ sessionId, operationId: report.operationId }); }
    catch { throw new FeedbackSendError(true); } // Lost bridge response: do not create a new report.
    if (generation !== this.generation) throw new Error('Feedback action cancelled.');
    if (!result.submitted) {
      if (!result.retryable) this.confirmed = null;
      throw new FeedbackSendError(result.retryable === true);
    }
    this.confirmed = null;
  }
}

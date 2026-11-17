import React from 'react';
import { renderToStaticMarkup } from 'react-dom/server';
import { expect, it } from 'vitest';
import { FeedbackPanel } from './FeedbackPanel';
it('renders opt-in off and no editable fields or enabled send control initially', () => {
  const html = renderToStaticMarkup(<FeedbackPanel/>);
  expect(html).toContain('nothing leaves this phone');
  expect(html).toContain('type="checkbox"');
  expect(html).not.toContain('checked=""');
  expect(html).not.toContain('<textarea');
  expect(html).not.toContain('Send report');
});

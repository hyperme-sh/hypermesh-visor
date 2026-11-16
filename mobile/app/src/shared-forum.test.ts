import { expect, it } from 'vitest';
import { createHash } from 'node:crypto';
import { readFileSync } from 'node:fs';
import { createElement } from 'react';
import { renderToStaticMarkup } from 'react-dom/server';
import { MessageComposer } from '@s3rch/forum-ui';
it('uses the recorded shared package bytes and native controlled composer contract',()=>{
  const root = new URL('../vendor/s3rch-forum-ui/', import.meta.url);
  const source = JSON.parse(readFileSync(new URL('UPSTREAM.json',root),'utf8'));
  expect(source.repository).toBe('FyberLabs/s3r.ch');
  expect(source.revision).toMatch(/^[a-f0-9]{40}$/);
  expect(source.source_version).toMatch(/^\d+\.\d+\.\d+$/);
  for (const [path, digest] of Object.entries(source.files)) {
    expect(createHash('sha256').update(readFileSync(new URL(path,root))).digest('hex')).toBe(digest);
  }
  const html = renderToStaticMarkup(createElement(MessageComposer, {
    value:'Kept draft', onChange:()=>{}, onSend:()=>{throw Error('render cannot submit');},
    disabled:true,rows:4,maxLength:65536,sendLabel:'Send message',
  }));
  expect(html).toContain('<textarea'); expect(html).toContain('Kept draft');
  expect(html).toContain('disabled=""'); expect(html).not.toContain('action=');
});

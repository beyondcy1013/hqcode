import test from 'node:test';
import assert from 'node:assert/strict';
import {
  I18N,
  MOBILE_KEY_MAP,
  detectDefaultLang,
  escapeHtml,
  parseUnifiedDiffLines,
} from '../static/hq-helpers.js';

test('i18n en and zh expose identical key sets', () => {
  const enKeys = Object.keys(I18N.en).sort();
  const zhKeys = Object.keys(I18N.zh).sort();
  assert.deepEqual(enKeys, zhKeys);
});

test('detectDefaultLang defaults to en for non-Chinese locales', () => {
  assert.equal(detectDefaultLang('en-US'), 'en');
  assert.equal(detectDefaultLang('ja-JP'), 'en');
  assert.equal(detectDefaultLang('zh-CN'), 'zh');
  assert.equal(detectDefaultLang(undefined), 'en');
});

test('parseUnifiedDiffLines tracks line numbers and classifies diff lines', () => {
  const sample = [
    'diff --git a/a.rs b/a.rs',
    '--- a/a.rs',
    '+++ b/a.rs',
    '@@ -10,2 +10,3 @@ fn demo() {',
    ' let a = 1;',
    '-let b = 2;',
    '+let b = 3;',
    '+let c = 4;',
  ].join('\n');

  const rows = parseUnifiedDiffLines(sample);
  assert.equal(rows[0].kind, 'meta');
  assert.equal(rows[3].kind, 'hunk');
  assert.deepEqual(rows[4], { kind: 'ctx', oldNum: '10', newNum: '10', text: ' let a = 1;' });
  assert.deepEqual(rows[5], { kind: 'del', oldNum: '11', newNum: '', text: '-let b = 2;' });
  assert.deepEqual(rows[6], { kind: 'add', oldNum: '', newNum: '11', text: '+let b = 3;' });
  assert.deepEqual(rows[7], { kind: 'add', oldNum: '', newNum: '12', text: '+let c = 4;' });
});

test('escapeHtml neutralizes HTML injection characters', () => {
  assert.equal(escapeHtml('<script>"\'&'), '&lt;script&gt;&quot;&#39;&amp;');
});

test('MOBILE_KEY_MAP contains essential control sequences', () => {
  assert.equal(MOBILE_KEY_MAP.esc, '\x1b');
  assert.equal(MOBILE_KEY_MAP.ctrlc, '\x03');
  assert.equal(MOBILE_KEY_MAP.approve, 'y\r');
  assert.equal(MOBILE_KEY_MAP.reject, 'n\r');
});

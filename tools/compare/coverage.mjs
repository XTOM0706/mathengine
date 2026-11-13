import katex from 'katex';
import fs from 'fs';
const lines = fs.readFileSync(process.argv[2], 'utf8').split('\n').map(s => s.trim()).filter(Boolean);
let ok = 0, no = 0;
for (const s of lines) {
  try {
    katex.renderToString(s, { throwOnError: true, displayMode: true });
    console.log('OK           ' + s);
    ok++;
  } catch (e) {
    console.log('FAIL         ' + s + '   [' + (e.name || 'Error') + ']');
    no++;
  }
}
console.log(`--- 成功 ${ok} / 失败 ${no}`);

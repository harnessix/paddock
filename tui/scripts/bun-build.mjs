/* global Bun */
// Run by bun from compile.mjs: one single binary of dist/cli.js.
// Ink imports react-devtools-core only when DEV=true. The bundler hoists
// that import, so the plugin gives it an empty module.
// Usage: bun scripts/bun-build.mjs TARGET OUTFILE
const [target, outfile] = process.argv.slice(2);
const stub = {
  name: 'no-devtools',
  setup(build) {
    build.onResolve({ filter: /^react-devtools-core$/ }, () => ({ path: 'react-devtools-core', namespace: 'stub' }));
    build.onLoad({ filter: /.*/, namespace: 'stub' }, () => ({
      contents: 'export default { initialize() {}, connectToDevTools() {} };',
      loader: 'js',
    }));
  },
};
const result = await Bun.build({
  entrypoints: [new URL('../dist/cli.js', import.meta.url).pathname],
  compile: { target: `bun-${target}`, outfile },
  minify: true,
  plugins: [stub],
});
if (!result.success) {
  for (const log of result.logs) console.error(log);
  process.exit(1);
}

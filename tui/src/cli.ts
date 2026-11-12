#!/usr/bin/env node
// The entry of paddock-tui. React chooses its fast production code or its
// slow development code when it loads, from NODE_ENV. So the program loads
// with NODE_ENV=production, unless the caller set one. After the load the
// variable is as it was: paddock and the agents that it starts must not
// inherit a NODE_ENV that the user did not set.

const env = process.env;
const had = env['NODE_ENV'];
if (had === undefined) env['NODE_ENV'] = 'production';
const { run } = await import('./main.js');
if (had === undefined) delete env['NODE_ENV'];
run();

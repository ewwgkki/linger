// Lets `node --test ui/test/` work on Node versions that resolve a directory argument as a module.
for (const f of ['util', 'library', 'vlist', 'i18n']) import(`./${f}.test.mjs`);

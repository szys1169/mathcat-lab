// Compatibility entry point: validate the immutable parent source captured before edits.
if(process.argv.includes('--capture'))throw new Error('The 2.4.3 source baseline already exists; capture is not a validation action.');
await import('./audit-previous-version.mjs');

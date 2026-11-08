// A seam between the wizard and the plugin setup gate (Task 10 sets it). Imports nothing.
let gate = null;

export function setBeforeSpawn(fn) {
  gate = typeof fn === 'function' ? fn : null;
}

// -> { skip: [plugin names] } to start, or null when the person cancelled.
export async function beforeSpawn(profile) {
  if (!gate) return { skip: [] };
  const r = await gate(profile);
  return r && Array.isArray(r.skip) ? r : null;
}

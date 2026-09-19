// The caller supplies its freshly created private process group, never a user PID.
// A local deadline still terminates callbacks if editor IPC or process inspection fails.
const group = Number(process.argv[2]);
if (!Number.isSafeInteger(group) || group <= 1) { process.exit(1); }
const timer = setTimeout(() => {
  try { process.kill(-group, 'SIGKILL'); } catch { /* The private group exited. */ }
  process.exit(0);
}, 1050);
if (process.argv[3] === '--lifetime-pipe') {
  process.stdin.on('end', () => { clearTimeout(timer); process.exit(0); });
  process.stdin.resume();
}

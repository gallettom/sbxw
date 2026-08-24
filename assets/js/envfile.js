// ── Environment file (.sbxenv.yaml) ───────────────────────────────────────
//
// The pane bar's "Env" button, next to "SSH" and behaving like it: a card
// hanging off the button, about the sandbox in *this* pane, closed by Escape,
// by clicking away, or by pressing the button again.
//
// It sits there rather than in the ports dialog because it is the same kind of
// thing as the SSH card — something you open, read and copy, not a form you
// fill in — and because the sandbox it describes is the one you are looking at.
//
// The panel is a preview before it is a writer. The one value that cannot be
// derived correctly for every machine is the repository root: the workspace is
// written as `${SBXW_PROJECTS_ROOT:-/the/exporting/machine}/neos`, so a
// colleague who keeps repositories elsewhere sets the variable once instead of
// editing every project's file. Making that root editable *here*, with the
// resulting `workspace:` line visible below it, is the difference between a
// setting you can check and one you have to guess.
const envPop           = document.getElementById('env-pop');
const envfileRootInput = document.getElementById('envfile-root');
const envfileRootNote  = document.getElementById('envfile-root-note');
const envfileYaml      = document.getElementById('envfile-yaml');
const envfileDest      = document.getElementById('envfile-dest');
const envfileError     = document.getElementById('envfile-error');
const envfileOverwrite = document.getElementById('envfile-overwrite');

/// The button the card is currently hanging off, or null when it is closed.
/// Doubles as the "ignore this click" test for the close-on-outside handler.
let envAnchor = null;
/// Which sandbox the card is showing. Kept apart from `envAnchor` so a slow
/// response for a previous sandbox can be discarded instead of painted over
/// the current one.
let envfileTarget = null;

function envfileShowError(msg) {
  envfileError.textContent = msg;
  envfileError.classList.remove('hidden');
}

function envfileClearError() {
  envfileError.classList.add('hidden');
  envfileError.textContent = '';
}

/// Render (and optionally write) the file for `envfileTarget`.
///
/// `save` is the only thing that touches the disk; every other call is a
/// preview, which is why one endpoint serves both and the button that writes is
/// the one that says "Save file".
async function envfileRender({ save = false } = {}) {
  const name = envfileTarget;
  if (!name) return;
  envfileClearError();
  const jobId = startBgJob(save ? `writing ${name}.sbxenv.yaml` : null);
  try {
    const res = await fetch(`/api/sandboxes/${encodeURIComponent(name)}/envfile`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({
        root: envfileRootInput.value.trim(),
        save,
        force: envfileOverwrite.checked,
      }),
    });
    const data = await res.json();
    // The card can be closed, or pointed at another sandbox, while this is in
    // flight.
    if (envfileTarget !== name) return;

    if (!data.ok) {
      envfileShowError(data.error || 'could not render the environment file');
      return;
    }
    envfileYaml.value = data.yaml;
    envfileDest.textContent = data.dest;
    envfileDest.title = data.dest;
    // Never rewrite the field under the cursor: the server echoes back the root
    // it actually used, which is not what someone half-way through typing one
    // wants to see appear in front of them.
    if (document.activeElement !== envfileRootInput) {
      envfileRootInput.value = data.projectsRoot;
    }
    envfileRootNote.textContent = data.rootFromEnv
      ? `From $${data.rootVar}. The workspace is written against it, with this path as the fallback.`
      : `No $${data.rootVar} set, so this is the workspace's parent. Set the variable `
        + `(install.sh offers to) and every project's file becomes portable.`;

    if (data.written) {
      showToast(`Wrote ${data.written}`, 'ok');
      envfileOverwrite.checked = false;
    }
    // The card grew or shrank by however much YAML came back; re-place it so it
    // doesn't hang off the bottom of the window.
    if (envAnchor) positionEnvPop(envAnchor);
  } catch (e) {
    if (envfileTarget === name) envfileShowError(`request failed: ${e}`);
  } finally {
    finishBgJob(jobId);
  }
}

function positionEnvPop(anchor) {
  if (!positionPopover(envPop, anchor)) closeEnvPop();
}

function toggleEnvPop(sandbox, anchor) {
  if (envAnchor === anchor && !envPop.classList.contains('hidden')) {
    closeEnvPop();
    return;
  }
  openEnvPop(sandbox, anchor);
}

function openEnvPop(sandbox, anchor) {
  envfileTarget = sandbox;
  envAnchor = anchor;
  document.getElementById('env-pop-title').textContent = `Environment file — ${sandbox}`;
  // Cleared first, then filled by the render: a slow response must never show
  // the previous sandbox's file under the new title.
  envfileYaml.value = '';
  envfileDest.textContent = '';
  envfileDest.title = '';
  envfileRootNote.textContent = '';
  envfileRootInput.value = '';
  envfileOverwrite.checked = false;
  envfileClearError();
  positionEnvPop(anchor);
  envfileRender();
}

function closeEnvPop() {
  envPop.classList.add('hidden');
  envAnchor = null;
  envfileTarget = null;
}

document.getElementById('env-pop-close').addEventListener('click', closeEnvPop);
document.getElementById('envfile-preview').addEventListener('click', () => envfileRender());
document.getElementById('envfile-save').addEventListener('click', () => envfileRender({ save: true }));
document.getElementById('envfile-copy').addEventListener('click', e => {
  if (!envfileYaml.value) return;
  copyField(e.currentTarget, envfileYaml.value);
});
// Enter in the root field means "show me what that changes", not "submit".
envfileRootInput.addEventListener('keydown', e => {
  if (e.key === 'Enter') { e.preventDefault(); envfileRender(); }
});

// Close on a click anywhere else. `pointerdown` on the capture phase so it
// still fires over the terminal, which swallows its own mouse events; the
// anchor is excluded because its own handler already toggles.
document.addEventListener('pointerdown', e => {
  if (!envAnchor) return;
  if (envPop.contains(e.target) || envAnchor.contains(e.target)) return;
  closeEnvPop();
}, true);
document.addEventListener('keydown', e => {
  if (e.key === 'Escape' && envAnchor) closeEnvPop();
});
// A card pinned to fixed coordinates goes stale the moment the layout moves.
window.addEventListener('resize', () => { if (envAnchor) positionEnvPop(envAnchor); });

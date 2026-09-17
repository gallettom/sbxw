// ── Environment file (sbxenv.yaml) ────────────────────────────────────────
//
// The pane bar's "Env" button, next to "SSH" and behaving like it: a card
// hanging off the button, about the sandbox in *this* pane, closed by Escape,
// by clicking away, or by pressing the button again.
//
// It sits there rather than in the ports dialog because it is the same kind of
// thing as the SSH card — something you open, read and copy, not a form you
// fill in — and because the sandbox it describes is the one you are looking at.
//
// The panel is a preview before it is a writer. The one value worth choosing is
// the folder the file goes in: sbx (0.43+) resolves the file's paths against
// the file itself, so the workspace is written as `./neos` relative to that
// folder, and the file works on any machine with the same layout below it.
// Making the folder editable *here*, with the resulting `workspace:` line
// visible below it, is the difference between a setting you can check and one
// you have to guess.
const envPop           = document.getElementById('env-pop');
const envfileRootInput = document.getElementById('envfile-root');
const envfileRootNote  = document.getElementById('envfile-root-note');
const envfileYaml      = document.getElementById('envfile-yaml');
const envfileDest      = document.getElementById('envfile-dest');
const envfileError     = document.getElementById('envfile-error');
const envfileOverwrite = document.getElementById('envfile-overwrite');

/// Which sandbox the card is showing. A slow response for a previous sandbox is
/// discarded rather than painted over the current one, so this is checked again
/// after every await.
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
  const jobId = startBgJob(save ? `writing sbxenv.yaml for ${name}` : null);
  try {
    const res = await fetch(`/api/sandboxes/${encodeURIComponent(name)}/envfile`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({
        dir: envfileRootInput.value.trim(),
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
    // Never rewrite the field under the cursor: the server echoes back the
    // folder it actually used, which is not what someone half-way through
    // typing one wants to see appear in front of them.
    const asked = envfileRootInput.value.trim().replace(/\/+$/, '');
    if (document.activeElement !== envfileRootInput) {
      envfileRootInput.value = data.dir;
    }
    envfileRootNote.textContent = asked && asked !== data.dir
      ? `${asked} does not contain the workspace, so the file goes in its parent instead.`
      : 'Paths are written relative to this folder. Any folder above the workspace will do.';

    if (data.written) {
      showToast(`Wrote ${data.written}`, 'ok');
      envfileOverwrite.checked = false;
    }
    // The card grew or shrank by however much YAML came back; re-place it so it
    // doesn't hang off the bottom of the window.
    envCard.reposition();
  } catch (e) {
    if (envfileTarget === name) envfileShowError(`request failed: ${e}`);
  } finally {
    finishBgJob(jobId);
  }
}

const envCard = bindPopover(envPop, {
  closeButton: 'env-pop-close',
  onOpen: sandbox => {
    envfileTarget = sandbox;
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
    envfileRender();
  },
  onClose: () => { envfileTarget = null; },
});

function toggleEnvPop(sandbox, anchor) { envCard.toggle(anchor, sandbox); }

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

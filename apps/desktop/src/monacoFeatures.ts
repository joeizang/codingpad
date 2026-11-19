// `editor.api.js` is Monaco's bare API: it has no suggest widget, hover, word
// commands or clipboard actions. Without them completion and hover providers
// are never asked, and keys such as Ctrl+Backspace fall through to the hidden
// textarea, which leaves Monaco's cursor out of step with its text.
// Register the editor contributions CodingPad uses, without the bundled
// language services that `editor.main.js` would also load.
import "monaco-editor/features/codicon/register.js";
import "monaco-editor/editor/browser/coreCommands.js";
import "monaco-editor/editor/common/standaloneStrings.js";

// Editing keys and commands.
import "monaco-editor/editor/contrib/wordOperations/browser/wordOperations.js";
import "monaco-editor/editor/contrib/wordPartOperations/browser/wordPartOperations.js";
import "monaco-editor/editor/contrib/linesOperations/browser/linesOperations.js";
import "monaco-editor/editor/contrib/multicursor/browser/multicursor.js";
import "monaco-editor/editor/contrib/cursorUndo/browser/cursorUndo.js";
import "monaco-editor/editor/contrib/clipboard/browser/clipboard.js";
import "monaco-editor/editor/contrib/dropOrPasteInto/browser/copyPasteContribution.js";
import "monaco-editor/editor/contrib/comment/browser/comment.js";
import "monaco-editor/editor/contrib/indentation/browser/indentation.js";
import "monaco-editor/editor/contrib/caretOperations/browser/caretOperations.js";
import "monaco-editor/editor/contrib/caretOperations/browser/transpose.js";
import "monaco-editor/editor/contrib/smartSelect/browser/smartSelect.js";
import "monaco-editor/editor/contrib/contextmenu/browser/contextmenu.js";

// Navigation and display.
import "monaco-editor/features/find/register.js";
import "monaco-editor/editor/contrib/find/browser/findController.js";
import "monaco-editor/editor/contrib/bracketMatching/browser/bracketMatching.js";
import "monaco-editor/editor/contrib/folding/browser/folding.js";
import "monaco-editor/editor/contrib/wordHighlighter/browser/wordHighlighter.js";
import "monaco-editor/editor/contrib/gotoError/browser/gotoError.js";
import "monaco-editor/editor/contrib/gotoError/browser/markerSelectionStatus.js";

// Code intelligence: completion widget, snippet insertion and hover.
import "monaco-editor/editor/contrib/snippet/browser/snippetController2.js";
import "monaco-editor/editor/contrib/suggest/browser/suggestController.js";
import "monaco-editor/editor/contrib/hover/browser/hoverContribution.js";

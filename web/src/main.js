import { EditorView, basicSetup } from "codemirror"
import { keymap } from '@codemirror/view'
import { HighlightStyle, syntaxHighlighting } from '@codemirror/language'
import { setDiagnostics } from '@codemirror/lint'
import { tags as t } from '@lezer/highlight'
import { dolphinLanguage } from './dolphin-lang.js'
import hello from '../../examples/hello.dol?raw'
import closure from '../../examples/closure.dol?raw'
import json from '../../examples/json.dol?raw'
import mandelbrot from '../../examples/mandelbrot.dol?raw'
import count from '../../examples/count.dol?raw'

const TIMEOUT_MS = 5000

const samples = { hello, closure, json, mandelbrot, count }

const highlight = HighlightStyle.define([
  { tag: t.keyword, color: '#c678dd' },
  { tag: t.string, color: '#98c379' },
  { tag: t.number, color: '#d19a66' },
  { tag: t.comment, color: '#7f8a99', fontStyle: 'italic' },
  { tag: t.variableName, color: '#61afef' },
  { tag: t.function(t.variableName), color: '#e5c07b' },
  { tag: t.atom, color: '#d19a66' },
  { tag: t.operator, color: '#56b6c2' },
])

const $ = (id) => document.getElementById(id)
const output = $('output')
const runButton = $('run')
const select = $('sample')

let worker
let timer

const view = new EditorView({
  parent: $('editor'),
  doc: hello,
  extensions: [
    basicSetup,
    dolphinLanguage,
    syntaxHighlighting(highlight),
    keymap.of([{ key: 'Mod-Enter', run: () => (run(), true) }]),
  ],
})

function show(text, isError) {
  output.textContent = text
  output.classList.toggle('error', isError)
}

function finish() {
  clearTimeout(timer)
  runButton.disabled = false
}

function start() {
  worker = new Worker(new URL('./worker.js', import.meta.url), { type: 'module' })
  worker.onmessage = ({ data }) => {
    finish()
    const { output: out, error, line } = data
    show(error ? out + error : out, !!error)
    const diags = []
    if (error && line > 0 && line <= view.state.doc.lines) {
      const l = view.state.doc.line(line)
      diags.push({ from: l.from, to: l.to, severity: 'error', message: error.split('\n')[0] })
    }
    view.dispatch(setDiagnostics(view.state, diags))
  }
}

function run() {
  if (runButton.disabled) return
  runButton.disabled = true
  show('実行中…', false)
  view.dispatch(setDiagnostics(view.state, []))
  timer = setTimeout(() => {
    worker.terminate()
    start()
    finish()
    show(`${TIMEOUT_MS / 1000} 秒を超えたので止めました（無限ループかもしれません）`, true)
  }, TIMEOUT_MS)
  worker.postMessage(view.state.doc.toString())
}

for (const name of Object.keys(samples)) select.add(new Option(`${name}.dol`, name))
select.onchange = () => {
  view.dispatch({ changes: { from: 0, to: view.state.doc.length, insert: samples[select.value] } })
  show('', false)
}
runButton.onclick = run

start()

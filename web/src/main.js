import { EditorView, basicSetup } from "codemirror"
import { keymap } from '@codemirror/view'
import { HighlightStyle, syntaxHighlighting } from '@codemirror/language'
import { setDiagnostics } from '@codemirror/lint'
import { tags as t } from '@lezer/highlight'
import { dolphinLanguage } from './dolphin-lang.js'
import hello from '../samples/hello.dol?raw'
import closure from '../../examples/closure.dol?raw'
import mandelbrot from '../../examples/mandelbrot.dol?raw'
import pong from '../../examples/pong.dol?raw'
import breakout from '../../examples/breakout.dol?raw'
import { Screen } from './screen.js'

const TIMEOUT_MS = 5000

// 先頭が最初に開く（開いたらすぐ実行する）
const samples = { breakout, pong, hello, closure, mandelbrot }

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
let lastError = ''
const screen = new Screen($('screen'))

const view = new EditorView({
  parent: $('editor'),
  doc: samples.breakout,
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

function append(text) {
  if (!text) return
  output.textContent += text
  output.scrollTop = output.scrollHeight
}

function finish() {
  clearTimeout(timer)
  runButton.disabled = false
}

function mark(error, line) {
  const diags = []
  if (error && line > 0 && line <= view.state.doc.lines) {
    const l = view.state.doc.line(line)
    diags.push({ from: l.from, to: l.to, severity: 'error', message: error.split('\n')[0] })
  }
  view.dispatch(setDiagnostics(view.state, diags))
}

// 1 回の実行・1 フレームが長すぎたら、worker ごと止める
function watchdog() {
  clearTimeout(timer)
  timer = setTimeout(() => {
    screen.close()
    start()
    finish()
    show(`${TIMEOUT_MS / 1000} 秒を超えたので止めました（無限ループかもしれません）`, true)
  }, TIMEOUT_MS)
}

function start() {
  worker?.terminate()
  worker = new Worker(new URL('./worker.js', import.meta.url), { type: 'module' })
  worker.onmessage = ({ data }) => {
    if (data.type === 'result') {
      const { output: out, error, line } = data
      show(error ? out + error : out, !!error)
      mark(error, line)
      if (data.canvas && !error) {
        // 画面があるうちは、フレームごとに見張る
        screen.open(data, (dt, input) => {
          watchdog()
          worker.postMessage({ type: 'frame', dt, input })
        })
        runButton.disabled = false
        clearTimeout(timer)
      } else {
        finish()
      }
    } else if (data.type === 'frame') {
      clearTimeout(timer)
      append(data.output)
      if (data.error && data.error !== lastError) append(`${data.error}\n`)
      lastError = data.error
      screen.receive(data.cmds)
      if (data.quit) screen.close()
    }
  }
}

function run() {
  if (runButton.disabled) return
  screen.close()
  $('stage').hidden = true
  lastError = ''
  start()
  runButton.disabled = true
  show('実行中…', false)
  mark('', 0)
  watchdog()
  worker.postMessage({ type: 'run', src: view.state.doc.toString() })
}

for (const name of Object.keys(samples)) select.add(new Option(`${name}.dol`, name))
select.onchange = () => {
  view.dispatch({ changes: { from: 0, to: view.state.doc.length, insert: samples[select.value] } })
  run()
}
runButton.onclick = run

run()

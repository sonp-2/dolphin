// editors/vscode/syntaxes/dolphin.tmLanguage.json に合わせた色付け
import { StreamLanguage } from '@codemirror/language'

const keywords = /^(if|else|while|for|in|break|continue|async|await)\b/
const constants = /^(true|false|nil)\b/

const dolphin = {
  startState: () => ({ str: false }),
  token(stream, state) {
    if (state.str) {
      while (!stream.eol()) {
        const c = stream.next()
        if (c === '\\') stream.next()
        else if (c === '"') { state.str = false; break }
      }
      return 'string'
    }
    if (stream.eatSpace()) return null
    if (stream.match('//')) { stream.skipToEnd(); return 'comment' }
    if (stream.eat('"')) { state.str = true; return 'string' }
    if (stream.match(/^0[xX][0-9a-fA-F_]+|^0[bB][01_]+|^0[oO][0-7_]+|^\d[\d_]*(\.\d+)?([eE][+-]?\d+)?i?/)) return 'number'
    if (stream.match(/^@[A-Za-z_]\w*/)) return 'variableName'
    if (stream.match(keywords)) return 'keyword'
    if (stream.match(constants)) return 'atom'
    if (stream.match(/^[A-Za-z_]\w*(?=\[)/)) return 'function(variableName)'
    if (stream.match(/^[A-Za-z_]\w*/)) return 'propertyName'
    if (stream.match(/^(:=|==|!=|<=|>=|&&|\|\||\.\.\.)/) || stream.eat(/[=<>+\-*\/%!?#]/)) return 'operator'
    stream.next()
    return null
  },
}

export const dolphinLanguage = StreamLanguage.define(dolphin)

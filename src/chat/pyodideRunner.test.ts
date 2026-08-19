import { describe, expect, it } from 'vitest'
import { sanitizePythonInputName, wrapPythonUserCode } from './pyodideRunner'

describe('sanitizePythonInputName', () => {
  it('keeps CJK filenames and strips path / reserved characters', () => {
    expect(sanitizePythonInputName('销售报表.xlsx')).toBe('销售报表.xlsx')
    expect(sanitizePythonInputName('Q1 销售.csv')).toBe('Q1 销售.csv')
    expect(sanitizePythonInputName('../secret?.png')).toBe('secret_.png')
    expect(sanitizePythonInputName('chart.png')).toBe('chart.png')
    expect(sanitizePythonInputName('...')).toBe('input')
  })
})

describe('wrapPythonUserCode', () => {
  it('suppresses non-fatal dependency warnings before executing user code', () => {
    const wrapped = wrapPythonUserCode('print("ok")')

    expect(wrapped).toContain('import warnings as _kivio_warnings')
    expect(wrapped).toContain('DeprecationWarning')
    expect(wrapped).toContain('PendingDeprecationWarning')
    expect(wrapped).toContain('FutureWarning')
    expect(wrapped).toContain('ResourceWarning')
    expect(wrapped).toContain('_kivio_warnings.filterwarnings("ignore"')
    expect(wrapped).toContain('exec("print')
  })
})

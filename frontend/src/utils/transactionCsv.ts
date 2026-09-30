import type { TransactionEvent } from '../hooks/useTransactionHistory'

export function toCsv(transactions: TransactionEvent[]): string {
  const header = ['Date', 'Direction', 'Amount (XLM)', 'Counterparty', 'Memo', 'Transaction Hash']
  const rows = transactions.map((tx) => [
    tx.timestamp, tx.direction, tx.amount, tx.counterparty, tx.memo ?? '', tx.txHash,
  ])
  const escapeCell = (value: string) => {
    // Quoting alone does not stop spreadsheet formula evaluation. Preserve the
    // original whitespace, but force dangerous cells to be interpreted as text.
    const safe = /^\s*[=+@-]/u.test(value) || /^\s*[\t\r\n]/u.test(value)
      ? `'${value}` : value
    return `"${safe.replace(/"/g, '""')}"`
  }
  return [header, ...rows].map((row) => row.map((cell) => escapeCell(String(cell))).join(',')).join('\r\n')
}


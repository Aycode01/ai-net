import React from 'react';
import { useTranslation } from 'react-i18next';
import { Prism as SyntaxHighlighter } from 'react-syntax-highlighter';
import { vscDarkPlus } from 'react-syntax-highlighter/dist/esm/styles/prism';
import { CodingResult } from '../../types/agent';
import { getCodeDetails } from '../../utils/agentUtils';
import CopyButton from '../common/CopyButton';

interface Props {
  result: CodingResult | null | undefined;
  searchQuery?: string;
}

const CodingRenderer: React.FC<Props> = ({ result, searchQuery }) => {
  const { t } = useTranslation();
  const details = getCodeDetails(result);

  if (!details || !details.code) {
    return (
      <div
        className="empty-state"
        id="empty-coding"
        style={{
          padding: '24px',
          textAlign: 'center',
          color: 'var(--text-secondary)',
          background: 'var(--white-alpha-02)',
          borderRadius: '8px',
          border: '1px dashed var(--white-alpha-10)',
        }}
      >
        {t('agent.coding.empty')}
      </div>
    );
  }

  const matchesSearch =
    !searchQuery || details.code.toLowerCase().includes(searchQuery.toLowerCase());

  if (!matchesSearch) {
    return (
      <div
        style={{
          padding: '20px',
          textAlign: 'center',
          color: 'var(--text-secondary)',
          fontSize: '0.85rem',
          fontStyle: 'italic',
        }}
      >
        No code matching search filter "{searchQuery}".
      </div>
    );
  }

  return (
    <div
      className="coding-container"
      id="coding-output"
      style={{
        position: 'relative',
        borderRadius: '8px',
        overflow: 'hidden',
        border: '1px solid var(--white-alpha-10)',
        backgroundColor: 'var(--surface-black)',
      }}
    >
      <div
        style={{
          position: 'absolute',
          top: '12px',
          right: '12px',
          zIndex: 10,
        }}
      >
        <CopyButton
          text={details.code}
          label={t('agent.coding.copyCode')}
          copiedLabel={t('agent.coding.copied')}
        />
      </div>
      <SyntaxHighlighter
        language={details.language}
        style={vscDarkPlus}
        showLineNumbers
        customStyle={{
          margin: 0,
          padding: '16px 20px',
          fontSize: '0.9rem',
          backgroundColor: 'transparent',
        }}
      >
        {details.code}
      </SyntaxHighlighter>
    </div>
  );
};

export default CodingRenderer;

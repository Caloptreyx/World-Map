import { faChevronDown, faChevronUp } from '@fortawesome/free-solid-svg-icons';
import { FontAwesomeIcon } from '@fortawesome/react-fontawesome';
import { type ReactNode, useState } from 'react';

interface Props {
  icon: ReactNode;
  title: string;
  /** Small controls on the title row (badges, icon buttons). */
  actions?: ReactNode;
  collapseLabel: string;
  children: ReactNode;
}

/** A glass panel floating over the map on wide screens; its body folds away to free the map. */
export default function FloatingPanel({ icon, title, actions, collapseLabel, children }: Props) {
  const [open, setOpen] = useState(true);
  return (
    <section className='wm-glass wm-panel'>
      <header className='wm-panel-title'>
        <span style={{ color: 'var(--mantine-color-dimmed)', display: 'inline-flex' }}>{icon}</span>
        <span style={{ flex: 1 }}>{title}</span>
        {actions}
        <button
          type='button'
          className='wm-tool'
          style={{ width: 28, height: 28, fontSize: 12 }}
          aria-label={collapseLabel}
          aria-expanded={open}
          onClick={() => setOpen((current) => !current)}
        >
          <FontAwesomeIcon icon={open ? faChevronUp : faChevronDown} />
        </button>
      </header>
      {open && <div style={{ marginTop: 10 }}>{children}</div>}
    </section>
  );
}

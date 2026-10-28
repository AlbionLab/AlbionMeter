import { useEffect, useId, useLayoutEffect, useRef, useState, type CSSProperties, type KeyboardEvent } from 'react';

export interface SelectOption { value: string; label: string; disabled?: boolean; badge?: string }

interface Props {
  value: string;
  options: SelectOption[];
  onChange: (value: string) => void;
  'aria-label'?: string;
  disabled?: boolean;
  className?: string;
  placeholder?: string;
}

export function Select({ value, options, onChange, disabled = false, className = '', placeholder = '', ...rest }: Props) {
  const [open, setOpen] = useState(false);
  const [active, setActive] = useState(-1);
  const [position, setPosition] = useState<CSSProperties>({});
  const button = useRef<HTMLButtonElement>(null);
  const list = useRef<HTMLUListElement>(null);
  const id = useId();
  const selected = options.find(option => option.value === value);

  const place = () => {
    const rect = button.current?.getBoundingClientRect();
    if (!rect) return;
    const height = Math.min(320, options.length * 36 + 10);
    const below = window.innerHeight - rect.bottom - 8;
    const up = below < height && rect.top > below;
    setPosition({ left: rect.left, minWidth: rect.width, maxWidth: Math.max(rect.width, window.innerWidth - rect.left - 8), ...(up ? { bottom: window.innerHeight - rect.top + 4 } : { top: rect.bottom + 4 }) });
  };
  const show = () => {
    if (disabled) return;
    place();
    setActive(Math.max(0, options.findIndex(option => option.value === value)));
    setOpen(true);
  };
  const choose = (option: SelectOption | undefined) => {
    if (!option || option.disabled) return;
    setOpen(false);
    button.current?.focus();
    if (option.value !== value) onChange(option.value);
  };
  const step = (from: number, direction: number) => {
    for (let index = from + direction; index >= 0 && index < options.length; index += direction) if (!options[index].disabled) return index;
    return from;
  };

  useLayoutEffect(() => { if (open) list.current?.querySelector<HTMLElement>('[data-active="true"]')?.scrollIntoView({ block: 'nearest' }); }, [open, active]);
  useEffect(() => {
    if (!open) return;
    const outside = (event: PointerEvent) => { if (!button.current?.contains(event.target as Node) && !list.current?.contains(event.target as Node)) setOpen(false); };
    const close = (event: Event) => { if (!list.current?.contains(event.target as Node)) setOpen(false); };
    document.addEventListener('pointerdown', outside);
    window.addEventListener('scroll', close, true);
    window.addEventListener('resize', close);
    return () => { document.removeEventListener('pointerdown', outside); window.removeEventListener('scroll', close, true); window.removeEventListener('resize', close); };
  }, [open]);

  const key = (event: KeyboardEvent) => {
    if (event.key === 'ArrowDown' || event.key === 'ArrowUp') {
      event.preventDefault();
      if (!open) { show(); return; }
      setActive(current => step(current, event.key === 'ArrowDown' ? 1 : -1));
    } else if (event.key === 'Home' && open) { event.preventDefault(); setActive(step(-1, 1)); }
    else if (event.key === 'End' && open) { event.preventDefault(); setActive(step(options.length, -1)); }
    else if (event.key === 'Enter' || event.key === ' ') { event.preventDefault(); if (open) choose(options[active]); else show(); }
    else if (event.key === 'Escape' && open) { event.preventDefault(); setOpen(false); }
    else if (event.key === 'Tab') setOpen(false);
  };

  return (
    <span className={`ui-select ${open ? 'open' : ''} ${className}`}>
      <button ref={button} type="button" className="ui-select-button" disabled={disabled} aria-label={rest['aria-label']} aria-haspopup="listbox" aria-expanded={open} aria-controls={open ? id : undefined}
        onClick={() => open ? setOpen(false) : show()} onKeyDown={key}>
        <span className="ui-select-value">{selected?.label ?? placeholder}</span>
        {selected?.badge && <span className="ui-select-badge">{selected.badge}</span>}
        <svg className="ui-select-chevron" width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2.4" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true"><path d="m6 9 6 6 6-6" /></svg>
      </button>
      {open && <ul ref={list} id={id} role="listbox" className="ui-select-list" style={position} aria-label={rest['aria-label']} aria-activedescendant={`${id}-${active}`}>
        {options.map((option, index) => (
          <li key={option.value} id={`${id}-${index}`} role="option" aria-selected={option.value === value} aria-disabled={option.disabled || undefined}
            data-active={index === active} onPointerEnter={() => { if (!option.disabled) setActive(index); }} onClick={() => choose(option)}>
            <span>{option.label}</span>
            {option.badge && <span className="ui-select-badge">{option.badge}</span>}
            {option.value === value && <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2.6" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true"><path d="M20 6 9 17l-5-5" /></svg>}
          </li>
        ))}
      </ul>}
    </span>
  );
}

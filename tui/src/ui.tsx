// Building blocks: a pane with its title in the border, a clickable list
// row, a button, a line of text in a role.

import React, { useRef, type ReactNode } from 'react';
import { Box, Text, type DOMElement } from 'ink';
import { cells, cut, fit } from './clean.js';
import { usePointer, type Handlers, type Pointer } from './mouse.js';
import { accentColor, roleProps, useTheme, type Role } from './theme.js';

export function T(props: { role?: Role; bold?: boolean; inverse?: boolean; underline?: boolean; children: ReactNode; wrap?: 'truncate-end' | 'wrap' }): React.JSX.Element {
  const theme = useTheme();
  const colors = theme.plain ? {} : roleProps(theme, props.role ?? 'normal');
  return (
    <Text {...colors} bold={props.bold || colors.bold} inverse={props.inverse} underline={props.underline} wrap={props.wrap ?? 'truncate-end'}>
      {props.children}
    </Text>
  );
}

/** Inner size of a pane of the given outer size. */
export function inner(plain: boolean, width: number, height: number): { width: number; height: number } {
  return plain ? { width, height: Math.max(0, height - 1) } : { width: Math.max(0, width - 4), height: Math.max(0, height - 2) };
}

export function Pane(props: {
  title: string;
  focused: boolean;
  width: number;
  height: number;
  children: ReactNode;
  onWheel?: (dir: 1 | -1) => void;
  onClick?: () => void;
  onDoubleClick?: () => void;
  /** Words in the bottom border: a position, a count. */
  footer?: string;
}): React.JSX.Element {
  const theme = useTheme();
  const ref = useRef<DOMElement>(null);
  const pointer = usePointer(ref, { onWheel: props.onWheel, onClick: props.onClick, onDoubleClick: props.onDoubleClick });
  const { width, height, focused } = props;
  const foot = props.footer !== undefined;
  // A pane that a click selects (a card) lights up under the pointer.
  const lit = !focused && pointer.hovered && (props.onClick !== undefined || props.onDoubleClick !== undefined);
  if (theme.plain) {
    return (
      <Box ref={ref} flexDirection="column" width={width} height={height} overflow="hidden">
        <Text bold underline={lit}>{cut(`${focused ? '> ' : ''}${props.title}`, width)}</Text>
        <Box flexDirection="column" height={height - (foot ? 2 : 1)} overflow="hidden">
          <Box flexDirection="column" flexShrink={0}>
            {props.children}
          </Box>
        </Box>
        {foot ? <Text dimColor>{cut(props.footer ?? '', width)}</Text> : null}
      </Box>
    );
  }
  const accent = accentColor(theme);
  const border = focused ? { borderColor: accent } : lit ? {} : { borderDimColor: true };
  const title = cut(props.title, Math.max(0, width - 6));
  const fill = Math.max(0, width - 5 - cells(title));
  const color = focused ? { color: accent } : lit ? {} : { dimColor: true };
  const footer = cut(props.footer ?? '', Math.max(0, width - 6));
  return (
    <Box ref={ref} flexDirection="column" width={width} height={height} overflow="hidden">
      <Text wrap="truncate-end">
        <Text {...color}>{'╭─ '}</Text>
        <Text bold={focused || pointer.pressed} dimColor={!focused && !lit} underline={lit}>{title}</Text>
        <Text {...color}>{' ' + '─'.repeat(fill) + '╮'}</Text>
      </Text>
      <Box
        borderStyle="round"
        borderTop={false}
        borderBottom={!foot}
        {...border}
        flexDirection="column"
        width={width}
        height={height - (foot ? 2 : 1)}
        paddingX={1}
        overflow="hidden"
      >
        {/* Lines that do not fit are cut at the bottom. Without this box they would shrink to no height and lie over each other. */}
        <Box flexDirection="column" flexShrink={0}>
          {props.children}
        </Box>
      </Box>
      {foot ? (
        <Text wrap="truncate-end">
          <Text {...color}>{footer ? '╰─ ' : '╰'}</Text>
          <Text dimColor={!focused}>{footer}</Text>
          <Text {...color}>{footer ? ' ' + '─'.repeat(Math.max(0, width - 5 - cells(footer))) + '╯' : '─'.repeat(Math.max(0, width - 2)) + '╯'}</Text>
        </Text>
      ) : null}
    </Box>
  );
}

export function Clickable(props: Handlers & { children: (hovered: boolean, pointer: Pointer) => ReactNode; width?: number }): React.JSX.Element {
  const ref = useRef<DOMElement>(null);
  const pointer = usePointer(ref, props);
  return (
    <Box ref={ref} width={props.width} flexShrink={0}>
      {props.children(pointer.hovered, pointer)}
    </Box>
  );
}

export function Button(props: {
  label: string;
  focused: boolean;
  role?: Role;
  onClick: () => void;
}): React.JSX.Element {
  return (
    <Clickable onClick={props.onClick}>
      {(hovered, pointer) => (
        // Under the pointer: underlined and bold. Held down: inverse, as the focused button, so the press shows before it acts.
        <T
          role={props.focused ? 'normal' : (props.role ?? 'normal')}
          inverse={props.focused || pointer.pressed}
          bold={props.focused || hovered}
          underline={hovered && !props.focused && !pointer.pressed}
        >
          {props.label}
        </T>
      )}
    </Clickable>
  );
}

export interface ListRow {
  key: string;
  /** The text of the row, already cut to width, and its role. */
  parts: { text: string; role?: Role; bold?: boolean }[];
}

/** The first visible row, so that `selected` stays in view. */
export function scrollTop(prev: number, selected: number, height: number, count: number): number {
  if (height <= 0) return 0;
  let top = Math.min(prev, Math.max(0, count - height));
  if (selected < top) top = selected;
  if (selected >= top + height) top = selected - height + 1;
  return Math.max(0, top);
}

export function List(props: {
  rows: ListRow[];
  selected: number;
  width: number;
  height: number;
  focused: boolean;
  empty: string;
  onSelect: (i: number) => void;
  onOpen: (i: number) => void;
}): React.JSX.Element {
  const top = useRef(0);
  const { rows, selected, width, height } = props;
  top.current = scrollTop(top.current, selected, height, rows.length);
  if (rows.length === 0) {
    return <T role="muted">{cut(props.empty, width)}</T>;
  }
  const visible = rows.slice(top.current, top.current + height);
  return (
    <Box flexDirection="column" width={width} height={height} overflow="hidden">
      {visible.map((r, i) => {
        const index = top.current + i;
        const sel = index === selected;
        return (
          <Clickable key={r.key} width={width} onClick={() => props.onSelect(index)} onDoubleClick={() => props.onOpen(index)}>
            {(hovered, pointer) => <RowText parts={r.parts} width={width} selected={sel} focused={props.focused} hovered={hovered} pressed={pointer.pressed} />}
          </Clickable>
        );
      })}
    </Box>
  );
}

function RowText(props: { parts: ListRow['parts']; width: number; selected: boolean; focused: boolean; hovered: boolean; pressed: boolean }): React.JSX.Element {
  const theme = useTheme();
  const marker = props.selected ? '› ' : '  ';
  let left = props.width - 2;
  const parts = props.parts.map((p) => {
    const t = cut(p.text, Math.max(0, left));
    left -= cells(t);
    return { ...p, text: t };
  });
  const pad = ' '.repeat(Math.max(0, left));
  const selColor = props.selected && props.focused ? { color: accentColor(theme) } : {};
  return (
    <Text wrap="truncate-end" inverse={(props.selected && props.focused) || props.pressed} underline={props.hovered && !props.pressed}>
      <Text {...selColor} bold={props.selected}>{marker}</Text>
      {parts.map((p, i) => (
        <Text key={i} {...(theme.plain ? {} : roleProps(theme, p.role ?? 'normal'))} bold={p.bold || props.selected || (theme.mono && (p.role === 'warning' || p.role === 'error' || p.role === 'accent'))}>
          {p.text}
        </Text>
      ))}
      <Text>{pad}</Text>
    </Text>
  );
}

export { fit };

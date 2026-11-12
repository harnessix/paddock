// Building blocks: a pane with its title in the border, a clickable list
// row, a button, a line of text in a role.

import React, { useRef, type ReactNode } from 'react';
import { Box, Text, type DOMElement } from 'ink';
import { cells, cut, fit } from './clean.js';
import { useClickable, type Handlers } from './mouse.js';
import { roleProps, useTheme, type Role } from './theme.js';

export function T(props: { role?: Role; bold?: boolean; inverse?: boolean; underline?: boolean; children: ReactNode; wrap?: 'truncate-end' | 'wrap' }): React.JSX.Element {
  const theme = useTheme();
  const colors = theme.plain ? {} : roleProps(theme, props.role ?? 'normal');
  return (
    <Text {...colors} bold={props.bold} inverse={props.inverse} underline={props.underline} wrap={props.wrap ?? 'truncate-end'}>
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
}): React.JSX.Element {
  const theme = useTheme();
  const ref = useRef<DOMElement>(null);
  useClickable(ref, { onWheel: props.onWheel, onClick: props.onClick });
  const { width, height, focused } = props;
  if (theme.plain) {
    return (
      <Box ref={ref} flexDirection="column" width={width} height={height} overflow="hidden">
        <Text bold>{cut(`${focused ? '> ' : ''}${props.title}`, width)}</Text>
        <Box flexDirection="column" height={height - 1} overflow="hidden">
          {props.children}
        </Box>
      </Box>
    );
  }
  const border = focused ? { borderColor: theme.color.accent } : { borderDimColor: true };
  const title = cut(props.title, Math.max(0, width - 6));
  const fill = Math.max(0, width - 5 - cells(title));
  const color = focused ? { color: theme.color.accent } : { dimColor: true };
  return (
    <Box ref={ref} flexDirection="column" width={width} height={height} overflow="hidden">
      <Text wrap="truncate-end">
        <Text {...color}>{'╭─ '}</Text>
        <Text bold={focused} dimColor={!focused}>{title}</Text>
        <Text {...color}>{' ' + '─'.repeat(fill) + '╮'}</Text>
      </Text>
      <Box
        borderStyle="round"
        borderTop={false}
        {...border}
        flexDirection="column"
        width={width}
        height={height - 1}
        paddingX={1}
        overflow="hidden"
      >
        {props.children}
      </Box>
    </Box>
  );
}

export function Clickable(props: Handlers & { children: (hovered: boolean) => ReactNode; width?: number }): React.JSX.Element {
  const ref = useRef<DOMElement>(null);
  const hovered = useClickable(ref, props);
  return (
    <Box ref={ref} width={props.width} flexShrink={0}>
      {props.children(hovered)}
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
      {(hovered) => (
        <T role={props.focused ? 'normal' : (props.role ?? 'normal')} inverse={props.focused} bold={props.focused} underline={hovered && !props.focused}>
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
            {(hovered) => <RowText parts={r.parts} width={width} selected={sel} focused={props.focused} hovered={hovered} />}
          </Clickable>
        );
      })}
    </Box>
  );
}

function RowText(props: { parts: ListRow['parts']; width: number; selected: boolean; focused: boolean; hovered: boolean }): React.JSX.Element {
  const theme = useTheme();
  const marker = props.selected ? '› ' : '  ';
  let left = props.width - 2;
  const parts = props.parts.map((p) => {
    const t = cut(p.text, Math.max(0, left));
    left -= cells(t);
    return { ...p, text: t };
  });
  const pad = ' '.repeat(Math.max(0, left));
  const selColor = props.selected && props.focused && !theme.plain ? { color: theme.color.accent } : {};
  return (
    <Text wrap="truncate-end" inverse={props.selected && props.focused} underline={props.hovered}>
      <Text {...selColor} bold={props.selected}>{marker}</Text>
      {parts.map((p, i) => (
        <Text key={i} {...(theme.plain ? {} : roleProps(theme, p.role ?? 'normal'))} bold={p.bold || props.selected}>
          {p.text}
        </Text>
      ))}
      <Text>{pad}</Text>
    </Text>
  );
}

export { fit };

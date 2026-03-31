
import { $ } from '@qwik.dev/core';
import { source } from 'lib';

const { a } = source;

export const handler = $(() => {
  console.log(a);
});

		
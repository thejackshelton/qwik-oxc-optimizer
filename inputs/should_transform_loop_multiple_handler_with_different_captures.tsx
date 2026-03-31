
import { component$, useSignal } from '@qwik.dev/core';

export default component$(() => {
  const arr = useSignal(['a', 'b']);
  return (
    <div>
      {arr.value.map((val, i) => {
        const plusOne = i + 1;
        const plusTwo = i + 2;
        const double = i * 2;
        return (
          <div
            onClick$={() => console.log(plusOne, double)}
            onKeyDown$={() => console.log(plusTwo, double)}
          >
            {val}
          </div>
        );
      })}
    </div>
  );
});

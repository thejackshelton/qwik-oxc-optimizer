
import { component$, useSignal } from '@qwik.dev/core';

export default component$(() => {
  const matrix = useSignal([['a', 'b'], ['c', 'd']]);
  return (
    <div>
      {matrix.value.map((row, i) => {
        const rowIndex = i + 1;
        return (
          <div key={i}>
            {row.map((cell, j) => {
              const cellIndex = j + 1;
              const cellKey = rowIndex + '-' + cellIndex;
              return (
                <span key={j}>
                  <button onClick$={() => console.log(rowIndex, cellIndex)}>{cell}</button>
                  <span onKeyDown$={() => console.log(cellKey, i, j)}>{cellKey}</span>
                </span>
              );
            })}
          </div>
        );
      })}
    </div>
  );
});

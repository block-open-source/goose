import React from 'react';
import ReactSelect from 'react-select';

export const Select = (props: React.ComponentProps<typeof ReactSelect>) => {
  return (
    <ReactSelect
      {...props}
      unstyled
      // react-select opens downwards by default, so a control near the bottom of the
      // window puts the last options past its edge. 'auto' flips the menu up instead.
      menuPlacement={props.menuPlacement ?? 'auto'}
      isSearchable={props.isSearchable !== false}
      closeMenuOnSelect={props.closeMenuOnSelect !== false}
      blurInputOnSelect={props.blurInputOnSelect !== false}
      classNames={{
        container: () => 'w-full cursor-pointer relative',
        indicatorSeparator: () => 'h-0',
        control: ({ isFocused }) =>
          `border ${isFocused ? 'border-border-primary' : 'border-border-primary'} focus:border-border-primary hover:border-border-primary rounded-md w-full px-4 py-2 text-sm text-text-secondary hover:cursor-pointer`,
        menu: () =>
          'mt-1 bg-background-primary border border-border-primary rounded-md text-text-secondary shadow-lg select__menu z-[9999] absolute',
        menuList: () => 'max-h-60 overflow-y-auto py-1',
        option: ({ isFocused, isSelected, isDisabled }) => {
          let classes = 'py-2 px-4 text-sm cursor-pointer';

          if (isDisabled) {
            classes += ' opacity-50 cursor-not-allowed pointer-events-none';
          } else if (isSelected) {
            classes += ' bg-background-inverse text-text-inverse pointer-events-auto';
          } else if (isFocused) {
            classes += ' bg-background-secondary text-text-primary pointer-events-auto';
          } else {
            classes += ' text-text-primary hover:bg-background-secondary pointer-events-auto';
          }

          return classes;
        },
      }}
      menuShouldBlockScroll={false}
      menuShouldScrollIntoView={false}
      tabSelectsValue={true}
      openMenuOnFocus={false}
      styles={{
        menu: (base) => ({
          ...base,
          pointerEvents: 'auto',
          zIndex: 9999,
        }),
        menuList: (base) => ({
          ...base,
          maxHeight: '240px',
          overflowY: 'auto',
          pointerEvents: 'auto',
        }),
        option: (base) => ({
          ...base,
          pointerEvents: 'auto',
          cursor: 'pointer',
        }),
      }}
    />
  );
};

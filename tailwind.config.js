/** @type {import('tailwindcss').Config} */
module.exports = {
  content: ["./templates/**/*.html"],
  theme: {
    extend: {
      fontFamily: {
        'sans': ['Inter', 'sans-serif']
      },
    },
  },
  plugins: [],
  safelist: [
    'bg-rose-50',
    'text-rose-700',
    'border-rose-500',
    'bg-emerald-50',
    'text-emerald-700',
    'border-emerald-500',
    'bg-sky-50',
    'text-sky-700',
    'border-sky-500',
    'animate-spin',
    'line-through',
  ],
}


## Fonts

This folder contains sources, instructions and license information for all the fonts used in the application.

### Material Symbols (+ subsetting instructions)

All icons in the project are sourced from Google's [Material Symbols](https://fonts.google.com/icons)
which are themselves licensed under [Apache 2.0](https://www.apache.org/licenses/LICENSE-2.0.html).
A copy of the license is provided in the `MaterialSymbolsRounded-LICENSE.txt` file.
Note that these terms apply to the redistribution of the font's source files - 
there are [no restrictions or limitations when using the font in a project](https://developers.google.com/fonts/faq#can_i_use_any_font_in_a_commercial_product).

Here is the list of all icon names used in the project:
- auto_delete
- bar_chart
- check
- check_circle
- clock_loader_90
- code
- content_paste
- data_usage
- delete
- download
- draft
- error
- folder_open
- home_storage
- key
- lock
- logout
- manage_accounts
- note_add
- open_in_new
- progress_activity
- public
- query_stats
- security
- upload_file
- visibility_off
- warning

The file `MaterialSymbolsRounded.woff` contains **all rounded symbols** and was sourced from the [offical GitHub repository](https://github.com/google/material-design-icons/tree/master/variablefont).
It is useful during development where new icons can directly be added in the frontend, but is completely untenable in production as it is 4.5MB large.

For production, a subsetted version of the font should be used.
Google themselves offer an API where a subsetted version of the font can be requested and downloaded:

```
https://fonts.googleapis.com/css2?family=Material+Symbols+Rounded&icon_names=auto_delete,bar_chart,check,check_circle,clock_loader_90,code,content_paste,data_usage,delete,download,draft,error,folder_open,home_storage,key,lock,logout,manage_accounts,note_add,open_in_new,progress_activity,public,query_stats,security,upload_file,visibility_off,warning
```

Should you want to add new icons to the project, add their identifier to the list and the link above, visit the link which shows you the automatically generated CSS file, visit the .woff2 URL embedded within, download the actual subsetted font, and finally add it here to the repository as `MaterialSymbolsRounded-subset.woff2`.


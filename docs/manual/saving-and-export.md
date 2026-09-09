# Saving and audio export

Save a project to keep editable music. Export audio to make a stereo file for listening or sharing. A synth preset or drum kit stores reusable sounds; it does not replace the project containing your notes, scenes, and arrangement.

## Save a project

1. Choose **File > Save As** for a new project or a named variation.
2. Enter a recognizable name and press Save.
3. Use **File > Save** as you continue editing.
4. Reopen through Projects in the sidebar or **File > Open Project**.

Save milestone versions before replacing a kit, restructuring an arrangement, or recording over a section you want to keep. Names such as Song sketch and Song arranged make those stages easy to recognize.

The app uses its projects folder. A project is editable music data, not a finished audio file. Keep the samples and other sound assets it relies on available when reopening it.

## Export the arrangement

1. Finish the arrangement's track clips and scene lane, then stop recording.
2. Play the section you intend to export. Check clip coverage, bus/group settings, and the ending.
3. Save the project so the arrangement you want is preserved.
4. Choose **File > Export Audio**.
5. Enter a file name and choose **Entire arrangement** or **Beat range**.
6. Choose the sample rate and a tail long enough for the final delay or reverb to decay.
7. Click **Export** and wait for completion.
8. Use **Show in Finder** on macOS to find the completed file in the displayed Recordings folder.

The export panel offers stereo 32-bit floating-point WAV at 44100, 48000, or 96000 Hz. Tail is measured in seconds and adds time for sound after the chosen musical end. If the completion message says audio remains at the end, try a longer tail.

Beat range uses beats counted from zero, not the ruler's displayed bar numbers. For example, eight bars of four beats starting at the beginning run from beat 0 to beat 32. End must be after Start. Check the units before entering a range.

An arrangement needs track content. Setting only a starting scene or scene marker does not place the notes to export. Use Place or Place Scene Patterns as described in [Arrangement](arrangement).

## Capture a live jam with WAV

Use the transport's **WAV** control when you want to capture the master output as you perform.

1. Turn WAV on before the performance.
2. Play patterns, launch sections, or perform notes and controls.
3. Allow the ending to decay, then turn WAV off to finish the file.
4. Check the app's saved-recording message and Recordings folder.

WAV is separate from the red note/arrangement Record control. It records sounding output; it does not create editable notes or arrangement take clips. Stopping the musical transport and finishing the WAV recording are separate actions, which lets you preserve a reverb tail.

Keep the project as well as the audio capture if you want to change notes, instruments, or the mix later. See [Recording](recording) for editable note takes and pattern overdubs.

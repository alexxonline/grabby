this approach only records system OUTPUT (what you hear from speakers), not your microphone input.
What Gets Recorded
WASAPI Loopback captures:

✅ Other participants' voices
✅ Shared audio/videos
✅ System sounds
❌ Your own microphone (NOT captured)

To Record BOTH Sides
You need to capture two separate audio streams and mix them:
Solution 1: Capture Both Streams Separately

import pyaudiowpatch as pyaudio
import wave
import numpy as np
from pydub import AudioSegment

class FullMeetingRecorder:
    def __init__(self):
        self.p = pyaudio.PyAudio()
        self.loopback_stream = None  # System audio
        self.mic_stream = None        # Your microphone
        self.loopback_frames = []
        self.mic_frames = []
        
    def get_loopback_device(self):
        """Get system audio loopback"""
        wasapi_info = self.p.get_host_api_info_by_type(pyaudio.paWASAPI)
        for i in range(self.p.get_device_count()):
            dev = self.p.get_device_info_by_index(i)
            if dev['hostApi'] == wasapi_info['index'] and \
               'loopback' in dev['name'].lower():
                return dev
        return None
    
    def get_default_microphone(self):
        """Get default microphone"""
        return self.p.get_default_input_device_info()
    
    def start_recording(self):
        """Start both streams"""
        loopback_dev = self.get_loopback_device()
        mic_dev = self.get_default_microphone()
        
        # Start loopback (system audio)
        self.loopback_stream = self.p.open(
            format=pyaudio.paInt16,
            channels=loopback_dev['maxInputChannels'],
            rate=int(loopback_dev['defaultSampleRate']),
            input=True,
            input_device_index=loopback_dev['index'],
            frames_per_buffer=1024
        )
        
        # Start microphone
        self.mic_stream = self.p.open(
            format=pyaudio.paInt16,
            channels=1,  # Mono for mic
            rate=48000,  # Match loopback rate
            input=True,
            input_device_index=mic_dev['index'],
            frames_per_buffer=1024
        )
        
        self.loopback_frames = []
        self.mic_frames = []
        print("Recording both streams...")
    
    def record_chunk(self):
        """Capture from both streams"""
        if self.loopback_stream and self.mic_stream:
            loopback_data = self.loopback_stream.read(1024, exception_on_overflow=False)
            mic_data = self.mic_stream.read(1024, exception_on_overflow=False)
            
            self.loopback_frames.append(loopback_data)
            self.mic_frames.append(mic_data)
    
    def stop_recording(self, output_mp3):
        """Mix and save both streams"""
        # Stop streams
        if self.loopback_stream:
            self.loopback_stream.stop_stream()
            self.loopback_stream.close()
        if self.mic_stream:
            self.mic_stream.stop_stream()
            self.mic_stream.close()
        
        # Save temporary WAV files
        loopback_wav = "temp_system.wav"
        mic_wav = "temp_mic.wav"
        
        # Save loopback
        with wave.open(loopback_wav, 'wb') as wf:
            wf.setnchannels(2)  # Stereo
            wf.setsampwidth(2)
            wf.setframerate(48000)
            wf.writeframes(b''.join(self.loopback_frames))
        
        # Save mic
        with wave.open(mic_wav, 'wb') as wf:
            wf.setnchannels(1)  # Mono
            wf.setsampwidth(2)
            wf.setframerate(48000)
            wf.writeframes(b''.join(self.mic_frames))
        
        # Mix using pydub
        system_audio = AudioSegment.from_wav(loopback_wav)
        mic_audio = AudioSegment.from_wav(mic_wav)
        
        # Convert mic to stereo and overlay
        mic_stereo = AudioSegment.from_mono_audiosegments(mic_audio, mic_audio)
        mixed = system_audio.overlay(mic_stereo)
        
        # Export to MP3
        mixed.export(output_mp3, format="mp3", bitrate="128k")
        
        print(f"Mixed recording saved to {output_mp3}")
        
        # Cleanup
        import os
        os.remove(loopback_wav)
        os.remove(mic_wav)


        Key Points

Two separate captures are needed: loopback (system) + microphone
Synchronization can be tricky - the streams may drift slightly
Mixing combines both into final output
Most meeting recorders work this way under the hood